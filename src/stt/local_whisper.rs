use std::path::Path;
use std::sync::Mutex;

use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState,
};

use crate::audio::{self, TARGET_RATE};
use crate::error::{Result, VoxError};
use crate::model::{AppContext, AudioChunk, Transcript};
use crate::stt::{SegmentHint, Transcriber};

const MAX_INITIAL_PROMPT_LEN: usize = 200;

/// whisper.cpp silently returns nothing for input under 1 s, which used to
/// swallow short replies like "Yes." or "Sounds good." entirely.
const MIN_AUDIO_SAMPLES: usize = (TARGET_RATE as usize * 5) / 4; // 1.25 s
/// Trailing silence so the last word isn't clipped mid-phoneme.
const TAIL_PADDING_SAMPLES: usize = TARGET_RATE as usize / 5; // 200 ms

pub struct LocalWhisper {
    ctx: WhisperContext,
    /// Reused across dictations: creating a state allocates the KV cache and
    /// compute buffers (tens of MB), which is pure overhead to repeat.
    state: Mutex<Option<WhisperState>>,
    english_only_model: bool,
    input_language: Option<String>,
    initial_prompt: Option<String>,
    threads: i32,
    /// The language last detected with confidence on a long enough clip;
    /// short, ambiguous clips fall back to it in auto mode.
    last_confident: Mutex<Option<&'static str>>,
}

impl LocalWhisper {
    pub fn new(model_path: impl AsRef<Path>) -> Result<Self> {
        let path = model_path.as_ref();
        if !path.exists() {
            return Err(VoxError::Transcription(format!(
                "whisper model not found at {}; run `vox download-model`",
                path.display()
            )));
        }
        let ctx = WhisperContext::new_with_params(
            path.to_string_lossy().as_ref(),
            WhisperContextParameters::default(),
        )
        .map_err(|e| VoxError::Transcription(e.to_string()))?;
        let english_only_model = path
            .file_name()
            .map(|n| n.to_string_lossy().contains(".en"))
            .unwrap_or(false);
        Ok(Self {
            ctx,
            state: Mutex::new(None),
            english_only_model,
            input_language: None,
            initial_prompt: None,
            threads: default_threads(),
            last_confident: Mutex::new(None),
        })
    }

    pub fn with_input_language(mut self, lang: impl Into<String>) -> Self {
        self.input_language = Some(lang.into());
        self
    }

    pub fn with_threads(mut self, threads: i32) -> Self {
        self.threads = threads.max(1);
        self
    }

    pub fn with_initial_prompt(mut self, dictionary: &[String]) -> Self {
        let joined: String = dictionary
            .iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(", ");
        if joined.is_empty() {
            self.initial_prompt = None;
        } else {
            let truncated = if joined.len() > MAX_INITIAL_PROMPT_LEN {
                joined.chars().take(MAX_INITIAL_PROMPT_LEN).collect()
            } else {
                joined
            };
            self.initial_prompt = Some(truncated);
        }
        self
    }

    /// Runs one throwaway inference so the first real dictation doesn't pay
    /// for lazy allocations and cold caches.
    pub fn warm_up(&self) {
        let silence_with_blip: Vec<f32> = (0..TARGET_RATE as usize)
            .map(|i| if i % 160 < 80 { 0.05 } else { -0.05 })
            .collect();
        let chunk = AudioChunk {
            samples: silence_with_blip,
            sample_rate: TARGET_RATE,
        };
        let _ = self.run(&chunk.samples, &SegmentHint::default(), None);
    }

    fn plan(&self) -> LanguagePlan {
        language_plan(self.input_language.as_deref(), self.english_only_model)
    }

    /// Picks the language for a clip that has no language from an earlier
    /// segment: detects it, restricted to the user's languages when they
    /// listed some. `None` means "let whisper.cpp auto-detect" (detection
    /// itself failed).
    fn detect_language(
        &self,
        state: &mut WhisperState,
        samples: &[f32],
        allowed: Option<&[&'static str]>,
    ) -> Option<&'static str> {
        let threads = self.threads.max(1) as usize;
        let probs = match state
            .pcm_to_mel(samples, threads)
            .and_then(|_| state.lang_detect(0, threads))
        {
            Ok((_, probs)) => probs,
            Err(e) => {
                tracing::debug!("language detection failed: {e}");
                return allowed.and_then(|a| a.first().copied());
            }
        };
        let ranked: Vec<(&'static str, f32)> = probs
            .iter()
            .enumerate()
            .filter_map(|(id, p)| whisper_rs::get_lang_str(id as i32).map(|code| (code, *p)))
            .collect();
        let secs = samples.len() as f32 / TARGET_RATE as f32;
        let mut sticky = self
            .last_confident
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let choice = pick_language(&ranked, allowed, secs, *sticky);
        if allowed.is_none() {
            if let Some(&(top, p)) = ranked.iter().max_by(|a, b| a.1.total_cmp(&b.1)) {
                if is_confident(secs, p) {
                    *sticky = Some(top);
                }
            }
        }
        tracing::debug!(
            "language: picked {choice:?} for {secs:.1}s clip (top: {:?})",
            top_languages(&ranked, 3)
        );
        choice
    }

    /// A later segment of a dictation in several languages: `pinned`
    /// unless this segment is long enough to tell and clearly another of
    /// the user's languages. Short segments (the tail after the key is
    /// released, usually) skip detection: it costs an encoder pass the
    /// user would wait through, and can't be trusted on them anyway.
    fn redetect(
        &self,
        state: &mut WhisperState,
        samples: &[f32],
        allowed: &[&'static str],
        pinned: &'static str,
    ) -> &'static str {
        let secs = samples.len() as f32 / TARGET_RATE as f32;
        if secs < SHORT_CLIP_SECS {
            return pinned;
        }
        let threads = self.threads.max(1) as usize;
        let probs = match state
            .pcm_to_mel(samples, threads)
            .and_then(|_| state.lang_detect(0, threads))
        {
            Ok((_, probs)) => probs,
            Err(e) => {
                tracing::debug!("language re-detection failed: {e}");
                return pinned;
            }
        };
        let ranked: Vec<(&'static str, f32)> = probs
            .iter()
            .enumerate()
            .filter_map(|(id, p)| whisper_rs::get_lang_str(id as i32).map(|code| (code, *p)))
            .collect();
        let choice = keep_or_switch(&ranked, allowed, secs, pinned);
        if choice != pinned {
            tracing::debug!("language: switched {pinned} -> {choice} for {secs:.1}s segment");
        }
        choice
    }

    /// One Whisper pass over `samples` in `language` (None: let whisper.cpp
    /// detect), with `prompt` as the preceding text. Returns the text and
    /// the mean probability of its word tokens.
    fn decode(
        &self,
        state: &mut WhisperState,
        samples: &[f32],
        language: Option<&str>,
        prompt: Option<&str>,
    ) -> Result<(String, Option<f32>)> {
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(self.threads);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        // Dictation never needs timestamps, and skipping their tokens makes
        // decoding measurably faster.
        params.set_no_timestamps(true);
        // Each dictation is independent; don't condition on the previous one.
        params.set_no_context(true);
        params.set_suppress_blank(true);
        params.set_language(Some(language.unwrap_or("auto")));
        if let Some(ctx) = audio_ctx_for(samples.len()) {
            params.set_audio_ctx(ctx);
        }
        // Fast speech is ~5 tokens/s; anything far beyond that is the
        // decoder looping, so bound it instead of letting it run to 224.
        let secs = samples.len() as f32 / TARGET_RATE as f32;
        params.set_max_tokens((secs * 10.0).ceil() as i32 + 16);
        if let Some(prompt) = prompt {
            params.set_initial_prompt(prompt);
        }

        let diagnostics = std::env::var_os("VOX_WHISPER_TIMINGS").is_some();
        if diagnostics {
            self.ctx.reset_timings();
        }
        let result = state.full(params, samples);
        if diagnostics {
            eprintln!("whisper system info: {}", whisper_rs::print_system_info());
            self.ctx.print_timings();
        }
        result.map_err(|e| VoxError::Transcription(e.to_string()))?;

        let n = state
            .full_n_segments()
            .map_err(|e| VoxError::Transcription(e.to_string()))?;
        let mut text = String::new();
        let eot = self.ctx.token_eot();
        let (mut prob_sum, mut tokens) = (0.0f32, 0u32);
        for i in 0..n {
            let seg = state
                .full_get_segment_text(i)
                .map_err(|e| VoxError::Transcription(e.to_string()))?;
            text.push_str(&seg);
            for j in 0..state.full_n_tokens(i).unwrap_or(0) {
                // Special tokens (timestamps, language, end of text) sit at
                // and above end-of-text; only the words count.
                let is_text = state.full_get_token_id(i, j).is_ok_and(|id| id < eot);
                if let (true, Ok(p)) = (is_text, state.full_get_token_prob(i, j)) {
                    prob_sum += p;
                    tokens += 1;
                }
            }
        }
        let confidence = (tokens > 0).then(|| prob_sum / tokens as f32);
        Ok((text, confidence))
    }

    /// Transcribes 16 kHz mono `samples`. `hint` carries the text and
    /// language of earlier segments when streaming (empty otherwise);
    /// `vocabulary` is extra spelling context, such as the window title.
    fn run(
        &self,
        samples: &[f32],
        hint: &SegmentHint,
        vocabulary: Option<&str>,
    ) -> Result<Transcript> {
        let mut padded;
        let samples = if samples.len() < MIN_AUDIO_SAMPLES {
            padded = samples.to_vec();
            padded.resize(MIN_AUDIO_SAMPLES + TAIL_PADDING_SAMPLES, 0.0);
            &padded[..]
        } else {
            padded = Vec::with_capacity(samples.len() + TAIL_PADDING_SAMPLES);
            padded.extend_from_slice(samples);
            padded.resize(samples.len() + TAIL_PADDING_SAMPLES, 0.0);
            &padded[..]
        };

        let mut guard = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if guard.is_none() {
            *guard = Some(
                self.ctx
                    .create_state()
                    .map_err(|e| VoxError::Transcription(e.to_string()))?,
            );
        }
        let state = guard.as_mut().expect("state initialized above");

        // whisper.cpp's own default is "en", so "auto" has to be passed
        // explicitly to auto-detect. Picking the language here instead costs
        // the same (whisper.cpp's auto mode runs the same detection pass)
        // but lets it be restricted to the languages the user speaks.
        let plan = self.plan();
        let allowed = plan.allowed();
        let earlier = hint
            .language
            .as_deref()
            .filter(|c| *c != "auto")
            .and_then(|c| whisper_rs::get_lang_id(c).and_then(whisper_rs::get_lang_str))
            .filter(|c| allowed.as_ref().is_none_or(|a| a.contains(c)));
        let language: Option<&'static str> = match &plan {
            LanguagePlan::Force(code) => Some(code),
            LanguagePlan::Hinglish => Some("en"),
            // Auto: keep the language an earlier segment settled on;
            // re-detecting among every language on a few-second segment
            // flips between related ones.
            LanguagePlan::Auto if earlier.is_some() => earlier,
            LanguagePlan::Auto => self.detect_language(state, samples, None),
            // Several languages: the user may switch mid-dictation, so look
            // again, but only leave the earlier language when this segment
            // is clearly another (see `keep_or_switch`).
            LanguagePlan::Constrained(codes) => match earlier {
                Some(pinned) => Some(self.redetect(state, samples, codes, pinned)),
                None => self.detect_language(state, samples, Some(codes)),
            },
        };

        let prompt = initial_prompt(
            self.initial_prompt.as_deref(),
            vocabulary,
            &hint.previous_text,
        );
        let (mut text, mut confidence) =
            match self.decode(state, samples, language, prompt.as_deref()) {
                Ok(decoded) => decoded,
                Err(e) => {
                    // A failed run can leave the state inconsistent; rebuild next time.
                    *guard = None;
                    return Err(e);
                }
            };

        // Whisper sometimes gives up on audio it finds hard (Hindi decoded as
        // English, say) and copies something from its prompt instead:
        // measured, 7-12 s of speech came back as "1.", "1.146.0" (from a
        // "Release Notes: 1.146.0" tab title) and "English". Output that
        // short can't be what was said, so decode once more with no prompt
        // and keep whichever says more. Only failures pay for the extra pass.
        let speech = &samples[..samples.len().saturating_sub(TAIL_PADDING_SAMPLES)];
        let speech_secs = speech.len() as f32 / TARGET_RATE as f32;
        let quiet = is_quiet(speech);
        if prompt.is_some() && !quiet && too_short_for(&text, speech_secs) {
            match self.decode(state, samples, language, None) {
                Ok((retry, retry_confidence)) if content_len(&retry) > content_len(&text) => {
                    tracing::warn!(
                        "{speech_secs:.1}s of speech decoded as {text:?}; without the prompt: {retry:?}"
                    );
                    text = retry;
                    confidence = retry_confidence;
                }
                Ok(_) => {}
                Err(e) => {
                    *guard = None;
                    return Err(e);
                }
            }
        }

        let detected = match language {
            Some(code) => Some(code.to_string()),
            None => state
                .full_lang_id_from_state()
                .ok()
                .and_then(whisper_rs::get_lang_str)
                .map(str::to_string),
        };
        let text = collapse_repeated_sentences(text.trim());

        // Whisper sometimes "transcribes" its own prompt on near-silent or
        // unclear audio. The prompt now holds the window title, so that
        // would paste an email subject or a document name as dictation.
        if is_prompt_echo(
            &text,
            &[self.initial_prompt.as_deref(), vocabulary],
            confidence,
            quiet,
        ) {
            tracing::warn!("discarded transcript that only repeats the prompt: {text:?}");
            return Ok(Transcript {
                text: String::new(),
                language: detected,
                confidence,
                rejected: None,
            });
        }

        // Never paste text in a script none of the user's languages use
        // (Arabic for someone who speaks English and Hindi).
        if let Some(script) = allowed
            .as_deref()
            .and_then(|a| super::script_mismatch(&text, a))
        {
            tracing::warn!("discarded transcript in {script} script: {text:?}");
            return Ok(Transcript {
                text: String::new(),
                language: detected,
                confidence,
                rejected: Some(format!("{script} script")),
            });
        }
        Ok(Transcript {
            text,
            language: detected,
            confidence,
            rejected: None,
        })
    }
}

/// How Whisper should treat the language of the audio.
#[derive(Debug, Clone, PartialEq, Eq)]
enum LanguagePlan {
    /// Decode as this language, no detection.
    Force(&'static str),
    /// Hinglish / code-switched Hindi-English: decode as Latin-script
    /// English, but output in Devanagari is still the user's language.
    Hinglish,
    /// Detect, but only among these (the languages the user speaks).
    Constrained(Vec<&'static str>),
    /// Detect among every language Whisper knows.
    Auto,
}

impl LanguagePlan {
    /// The languages output may be in, when the user said.
    fn allowed(&self) -> Option<Vec<&'static str>> {
        match self {
            LanguagePlan::Force(code) => Some(vec![code]),
            LanguagePlan::Hinglish => Some(vec!["en", "hi"]),
            LanguagePlan::Constrained(codes) => Some(codes.clone()),
            LanguagePlan::Auto => None,
        }
    }
}

/// Reads the input-language setting: "auto", one language, Hinglish, or
/// several languages ("English, Hindi", "en+hi").
fn language_plan(input: Option<&str>, english_only_model: bool) -> LanguagePlan {
    if english_only_model {
        return LanguagePlan::Force("en");
    }
    let lang = input.unwrap_or("auto").trim();
    let lower = lang.to_ascii_lowercase();
    if lower.is_empty() || lower == "auto" {
        return LanguagePlan::Auto;
    }
    // Hinglish/code-switched speech decodes best as Latin-script English
    // (what the cleanup prompt's few-shot examples expect); language ID
    // would flip between hi/ur/en per clip.
    if lower.contains("hinglish") || lower.contains("mixed") || lower.contains("code-switch") {
        return LanguagePlan::Hinglish;
    }
    let mut codes: Vec<&'static str> = Vec::new();
    for part in lower
        .split([',', '+', '/', '&', ';'])
        .flat_map(|p| p.split(" and "))
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        if let Some(code) = language_code_any(part) {
            if !codes.contains(&code) {
                codes.push(code);
            }
        }
    }
    match codes.len() {
        0 => LanguagePlan::Auto,
        1 => LanguagePlan::Force(codes[0]),
        _ => LanguagePlan::Constrained(codes),
    }
}

/// A whisper language code for a name or code, covering every language
/// Whisper knows ("Hindi", "hi", "marathi"), not just the common ones.
fn language_code_any(name: &str) -> Option<&'static str> {
    whisper_language_code(name).or_else(|| {
        let lower = name.trim().to_ascii_lowercase();
        whisper_rs::get_lang_id(&lower).and_then(whisper_rs::get_lang_str)
    })
}

/// Clips shorter than this carry too little speech to identify a language
/// reliably; a 1-2 s accented clip is where Whisper picks Arabic for Hindi.
const SHORT_CLIP_SECS: f32 = 3.0;
/// Below this top probability on a short clip, prefer the language the user
/// was confidently speaking before.
const UNSURE_PROB: f32 = 0.5;
/// A detection this sure, on a clip long enough, becomes the fallback.
const CONFIDENT_PROB: f32 = 0.7;

fn is_confident(secs: f32, prob: f32) -> bool {
    secs >= SHORT_CLIP_SECS && prob >= CONFIDENT_PROB
}

/// Chooses a language from `ranked` (code, probability): the most likely of
/// `allowed` when given; otherwise the most likely overall, unless the clip
/// is short and the detector unsure, in which case the user's last
/// confidently detected language (`sticky`) wins.
fn pick_language(
    ranked: &[(&'static str, f32)],
    allowed: Option<&[&'static str]>,
    secs: f32,
    sticky: Option<&'static str>,
) -> Option<&'static str> {
    let best = |pred: &dyn Fn(&str) -> bool| {
        ranked
            .iter()
            .filter(|(c, _)| pred(c))
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .copied()
    };
    match allowed {
        Some(codes) => best(&|c| codes.contains(&c))
            .map(|(c, _)| c)
            .or_else(|| codes.first().copied()),
        None => {
            let (top, p) = best(&|_| true)?;
            match sticky {
                Some(s) if secs < SHORT_CLIP_SECS && p < UNSURE_PROB => Some(s),
                _ => Some(top),
            }
        }
    }
}

/// For a later segment in a multi-language dictation: the most likely of
/// `allowed` if it differs from `pinned` and the detector is confident
/// (same bar as the sticky fallback), else `pinned`. Indian-accented
/// English often gets a fair "hi" score; decoding English as Hindi gives
/// Devanagari or a translation, which is worse than missing a switch.
fn keep_or_switch(
    ranked: &[(&'static str, f32)],
    allowed: &[&'static str],
    secs: f32,
    pinned: &'static str,
) -> &'static str {
    ranked
        .iter()
        .filter(|(c, _)| allowed.contains(c))
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .filter(|(c, p)| *c != pinned && is_confident(secs, *p))
        .map_or(pinned, |(c, _)| c)
}

/// Loudest 20 ms window below this is a mumble or room noise, not speech
/// someone meant to dictate (quiet speech at arm's length measures >0.02).
const QUIET_RMS: f32 = 0.02;
/// Below this mean token probability Whisper is mostly guessing.
const LOW_CONFIDENCE: f32 = 0.5;

/// Speech at least this long is worth a second pass when it decodes to
/// next to nothing.
const RETRY_MIN_SECS: f32 = 4.0;
/// Fewer letters and digits per second of speech than this is not a
/// transcript. Real speech runs ~10-15 in English or Hindi, ~4 in Chinese.
const MIN_CHARS_PER_SEC: f32 = 2.0;

/// Letters and digits in `text`: how much it says, in any script.
fn content_len(text: &str) -> usize {
    text.chars().filter(|c| c.is_alphanumeric()).count()
}

/// True when `text` is far too short to be `secs` of speech ("1." for 7 s).
fn too_short_for(text: &str, secs: f32) -> bool {
    secs >= RETRY_MIN_SECS && (content_len(text) as f32) < secs * MIN_CHARS_PER_SEC
}

fn is_quiet(samples: &[f32]) -> bool {
    audio::window_rms(samples, TARGET_RATE)
        .into_iter()
        .fold(0.0f32, f32::max)
        < QUIET_RMS
}

/// True when `text` is only a copy of what Whisper was prompted with: it
/// reproduces two or more prompt parts (dictionary entries, title parts)
/// in a row, or exactly one part while Whisper was unsure or the audio was
/// quiet. Saying "ICICI Lombard" clearly into the ICICI site's search box,
/// or "Vox" with Vox in the dictionary, is still dictation.
fn is_prompt_echo(
    text: &str,
    prompts: &[Option<&str>],
    confidence: Option<f32>,
    quiet: bool,
) -> bool {
    let words = |s: &str| -> Vec<String> {
        s.split_whitespace()
            .map(|w| {
                w.chars()
                    .filter(|c| c.is_alphanumeric())
                    .flat_map(char::to_lowercase)
                    .collect::<String>()
            })
            .filter(|w| !w.is_empty())
            .collect()
    };
    let said = words(text);
    if said.is_empty() {
        return false;
    }
    // Every prompt word, tagged with the part it came from.
    let mut context: Vec<(String, usize)> = Vec::new();
    let mut parts = 0usize;
    for prompt in prompts.iter().flatten() {
        for part in prompt.split(", ") {
            let w = words(part);
            if w.is_empty() {
                continue;
            }
            context.extend(w.into_iter().map(|w| (w, parts)));
            parts += 1;
        }
    }
    let unsure = confidence.is_some_and(|c| c < LOW_CONFIDENCE) || quiet;
    (0..context.len().saturating_sub(said.len() - 1)).any(|start| {
        let run = &context[start..start + said.len()];
        if !run.iter().zip(&said).all(|((c, _), s)| c == s) {
            return false;
        }
        let (first, last) = (run[0].1, run[run.len() - 1].1);
        // Starts and ends on part boundaries, so it's whole parts copied.
        let starts_part = start == 0 || context[start - 1].1 != first;
        let ends_part =
            start + said.len() == context.len() || context[start + said.len()].1 != last;
        if !(starts_part && ends_part) {
            return false;
        }
        // Two or more parts in a row, or one whole part on unsure audio.
        first != last || unsure
    })
}

fn top_languages(ranked: &[(&'static str, f32)], n: usize) -> Vec<(&'static str, f32)> {
    let mut sorted = ranked.to_vec();
    sorted.sort_by(|a, b| b.1.total_cmp(&a.1));
    sorted.truncate(n);
    sorted
}

/// Max length of the window-title hint added to Whisper's prompt.
const MAX_TITLE_HINT_LEN: usize = 80;

/// Words from the focused window's title that are worth spelling right
/// ("ICICI Lombard" from "List of Cashless Network Garages - ICICI Lombard -
/// Google Chrome"): the title minus the app's own name, unread counters,
/// and placeholder titles. Stays on the device like everything else.
pub fn title_vocabulary(title: Option<&str>, app_name: &str) -> Option<String> {
    const APP_SUFFIXES: &[&str] = &[
        "google chrome",
        "chrome",
        "mozilla firefox",
        "firefox",
        "microsoft edge",
        "edge",
        "brave",
        "safari",
        "arc",
        "opera",
        "visual studio code",
        "cursor",
        "windsurf",
    ];
    const PLACEHOLDERS: &[&str] = &["new tab", "welcome", "untitled", "start page", "home"];
    // Editor and browser page labels: not names anyone needs spelled.
    const UI_LABELS: &[&str] = &[
        "release notes",
        "settings",
        "extension",
        "extensions",
        "keyboard shortcuts",
        "problems",
        "output",
        "terminal",
        "search",
        "source control",
        "explorer",
        "preview",
        "get started",
        "walkthrough",
        "inbox",
        "notifications",
        "downloads",
        "history",
    ];
    let app = app_name.trim().to_ascii_lowercase();
    let normalized = title?.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut parts: Vec<String> = Vec::new();
    for raw in normalized
        .split(" - ")
        .flat_map(|p| p.split(" | "))
        .flat_map(|p| p.split(" — "))
    {
        let mut part = raw.trim();
        // "(61) Song name" - an unread/notification counter.
        if part.starts_with('(') {
            if let Some(end) = part.find(')') {
                if part[1..end].chars().all(|c| c.is_ascii_digit() || c == '+') {
                    part = part[end + 1..].trim();
                }
            }
        }
        // "Release Notes: 1.146.0", "Extension: Rust Analyzer": an editor's
        // own page label in front of the actual subject.
        if let Some((label, rest)) = part.split_once(':') {
            if UI_LABELS.contains(&label.trim().to_ascii_lowercase().as_str()) {
                part = rest.trim();
            }
        }
        // Only words need spelling help. Version numbers, dates and counts
        // ("1.146.0", "v2.3", "#684") don't, and Whisper has pasted
        // "1.146.0" from the title instead of the user's sentence.
        let words: Vec<&str> = part
            .split_whitespace()
            .filter(|w| {
                let letters = w.chars().filter(|c| c.is_alphabetic()).count();
                let digits = w.chars().filter(|c| c.is_ascii_digit()).count();
                letters > 0 && letters > digits
            })
            .collect();
        let part = words.join(" ");
        let lower = part.to_ascii_lowercase();
        if part.is_empty()
            || lower == app
            || APP_SUFFIXES.contains(&lower.as_str())
            || PLACEHOLDERS.contains(&lower.as_str())
            || UI_LABELS.contains(&lower.as_str())
        {
            continue;
        }
        if !parts.iter().any(|p| p.eq_ignore_ascii_case(&part)) {
            parts.push(part);
        }
    }
    let mut out = String::new();
    for part in parts {
        let sep = if out.is_empty() { 0 } else { 2 };
        if out.len() + sep + part.len() > MAX_TITLE_HINT_LEN {
            // Skip this one, not the rest: the site's name is usually the
            // short last part, after a long page title.
            continue;
        }
        if sep > 0 {
            out.push_str(", ");
        }
        out.push_str(&part);
    }
    (!out.is_empty()).then_some(out)
}

/// Whisper's prompt is "the text before this audio": the custom dictionary
/// (so names are spelled right), then words from the window title, then,
/// when streaming, the end of what was said so far (so the segment
/// continues the sentence rather than starting a new one with a capital).
fn initial_prompt(
    dictionary: Option<&str>,
    vocabulary: Option<&str>,
    previous_text: &str,
) -> Option<String> {
    let parts: Vec<&str> = [dictionary, vocabulary, Some(previous_text)]
        .into_iter()
        .flatten()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    (!parts.is_empty()).then(|| parts.join(". "))
}

/// Safety net for decoder loops, which are hallucinations, never dictation:
/// - one sentence three or more times in a row ("Yes. Yes. Yes.");
/// - a group of 2-4 sentences (4+ words) twice or more in a row, which is
///   how a loop looks on a longer clip (measured: three questions repeated
///   three times over a 5 s clip), plus the cut-off start of one more copy
///   at the end, where the token limit stopped the loop.
///
/// Keeps one copy. "Yes. Yes." and "Thanks! Bye. Thanks! Bye." stay.
pub fn collapse_repeated_sentences(text: &str) -> String {
    let mut sentences: Vec<&str> = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        // A boundary is .!? followed by whitespace or the end ("2.5" isn't).
        let at_boundary = chars.peek().is_none_or(|(_, n)| n.is_whitespace());
        if matches!(c, '.' | '!' | '?') && at_boundary {
            let end = i + c.len_utf8();
            let s = text[start..end].trim();
            if !s.is_empty() {
                sentences.push(s);
            }
            start = end;
        }
    }
    let rest = text[start..].trim();
    if !rest.is_empty() {
        sentences.push(rest);
    }

    const MAX_GROUP: usize = 4;
    const MIN_GROUP_WORDS: usize = 4;
    let n = sentences.len();
    let same = |a: usize, b: usize| sentences[a].eq_ignore_ascii_case(sentences[b]);
    let mut out: Vec<&str> = Vec::new();
    let mut collapsed = false;
    let mut i = 0;
    'outer: while i < n {
        // Smallest period first, so "A. A. A. A." is one sentence repeated,
        // not a pair repeated.
        for k in 1..=MAX_GROUP {
            let min_reps = if k == 1 { 3 } else { 2 };
            if i + k * min_reps > n {
                break;
            }
            let mut reps = 1;
            while i + (reps + 1) * k <= n && (0..k).all(|j| same(i + j, i + reps * k + j)) {
                reps += 1;
            }
            let words: usize = sentences[i..i + k]
                .iter()
                .map(|s| s.split_whitespace().count())
                .sum();
            if reps < min_reps || (k > 1 && words < MIN_GROUP_WORDS) {
                continue;
            }
            out.extend_from_slice(&sentences[i..i + k]);
            collapsed = true;
            i += reps * k;
            // The loop's last, truncated copy: whatever is left is the start
            // of the group.
            let rest = sentences[i..].join(" ").to_lowercase();
            let group = sentences[i - k..i].join(" ").to_lowercase();
            if !rest.is_empty() && group.starts_with(&rest) {
                i = n;
            }
            continue 'outer;
        }
        out.push(sentences[i]);
        i += 1;
    }
    if collapsed {
        out.join(" ")
    } else {
        text.to_string()
    }
}

/// Frames of margin past the end of the clip (128 = 2.6 s).
const AUDIO_CTX_MARGIN: i32 = 128;
/// Below ~6 s of context, very short clips ("Yes.") make the decoder loop
/// ("Yes. Yes. Yes. ..."); measured: 256 loops, 320 and up are clean.
const MIN_AUDIO_CTX: i32 = 384;

/// Whisper's encoder always attends over a 30 s window (1500 frames, 50 per
/// second) no matter how short the clip, so a 4 s dictation pays for 30 s of
/// attention - the dominant cost on CPU. Shrinking the context to the clip
/// plus margin made a 4.4 s dictation go from 6.8 s to ~0.7 s on a 4-core
/// laptop with identical text. `None` = full context (also forced with
/// VOX_FULL_AUDIO_CTX=1, in case a model misbehaves with it).
fn audio_ctx_for(samples: usize) -> Option<i32> {
    if std::env::var_os("VOX_FULL_AUDIO_CTX").is_some() {
        return None;
    }
    let frames = ((samples as f64 / TARGET_RATE as f64) * 50.0).ceil() as i32;
    let ctx = ((frames + AUDIO_CTX_MARGIN + 63) / 64 * 64).max(MIN_AUDIO_CTX);
    (ctx < 1500).then_some(ctx)
}

/// On x86 (2-way SMT), using every hyperthread oversubscribes the SIMD units
/// and using only one per core leaves throughput on the table; measured on a
/// 4-core/8-thread laptop: 4 threads 952 ms, 6 threads 599 ms, 8 threads
/// 747 ms for the same clip. Apple Silicon has no SMT: use the cores.
fn default_threads() -> i32 {
    if let Some(n) = std::env::var("VOX_WHISPER_THREADS")
        .ok()
        .and_then(|v| v.parse::<i32>().ok())
    {
        return n.max(1);
    }
    let logical = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let threads = if cfg!(target_arch = "x86_64") && logical >= 4 {
        logical * 3 / 4
    } else {
        logical
    };
    threads.clamp(1, 8) as i32
}

impl Transcriber for LocalWhisper {
    fn transcribe(&self, audio: &AudioChunk, ctx: &AppContext) -> Result<Transcript> {
        if audio.samples.is_empty() {
            return Ok(Transcript::new(""));
        }
        if audio.sample_rate != TARGET_RATE {
            return Err(VoxError::Transcription(format!(
                "expected sample rate {TARGET_RATE}, got {}",
                audio.sample_rate
            )));
        }
        // Nobody spoke: skip Whisper entirely (and its "Thank you." ghosts).
        if audio::is_silent(&audio.samples, audio.sample_rate) {
            return Ok(Transcript::new(""));
        }
        let vocabulary = title_vocabulary(ctx.window_title.as_deref(), &ctx.app_name);
        self.run(
            &audio.samples,
            &SegmentHint::default(),
            vocabulary.as_deref(),
        )
    }

    fn transcribe_segment(
        &self,
        audio: &AudioChunk,
        ctx: &AppContext,
        hint: &SegmentHint,
    ) -> Result<Transcript> {
        if audio.samples.is_empty() || audio::is_silent(&audio.samples, audio.sample_rate) {
            return Ok(Transcript::new(""));
        }
        if audio.sample_rate != TARGET_RATE {
            return Err(VoxError::Transcription(format!(
                "expected sample rate {TARGET_RATE}, got {}",
                audio.sample_rate
            )));
        }
        let vocabulary = title_vocabulary(ctx.window_title.as_deref(), &ctx.app_name);
        self.run(&audio.samples, hint, vocabulary.as_deref())
    }

    fn name(&self) -> &'static str {
        "local-whisper"
    }
}

pub fn whisper_language_code(name: &str) -> Option<&'static str> {
    match name.trim().to_ascii_lowercase().as_str() {
        "english" | "en" => Some("en"),
        "hindi" | "hi" => Some("hi"),
        "tamil" | "ta" => Some("ta"),
        "spanish" | "es" => Some("es"),
        "japanese" | "ja" => Some("ja"),
        "french" | "fr" => Some("fr"),
        "german" | "de" => Some("de"),
        "portuguese" | "pt" => Some("pt"),
        "chinese" | "zh" => Some("zh"),
        "arabic" | "ar" => Some("ar"),
        "korean" | "ko" => Some("ko"),
        "italian" | "it" => Some("it"),
        "russian" | "ru" => Some("ru"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_codes() {
        assert_eq!(whisper_language_code("English"), Some("en"));
        assert_eq!(whisper_language_code("auto"), None);
        assert_eq!(
            whisper_language_code("Hinglish / mixed Hindi-English"),
            None
        );
    }

    #[test]
    fn prompt_puts_dictionary_then_title_then_previous_text() {
        assert_eq!(initial_prompt(None, None, ""), None);
        assert_eq!(
            initial_prompt(Some("Vox, CI/CD"), None, " "),
            Some("Vox, CI/CD".into())
        );
        assert_eq!(
            initial_prompt(None, None, "we ship"),
            Some("we ship".into())
        );
        assert_eq!(
            initial_prompt(Some("Vox"), None, "we ship"),
            Some("Vox. we ship".into())
        );
        assert_eq!(
            initial_prompt(Some("Vox"), Some("ICICI Lombard"), "we ship"),
            Some("Vox. ICICI Lombard. we ship".into())
        );
        assert_eq!(initial_prompt(None, Some(" "), ""), None);
    }

    #[test]
    fn language_plan_reads_the_setting() {
        use LanguagePlan::*;
        assert_eq!(language_plan(None, false), Auto);
        assert_eq!(language_plan(Some("auto"), false), Auto);
        assert_eq!(language_plan(Some(" "), false), Auto);
        assert_eq!(language_plan(Some("Hindi"), false), Force("hi"));
        assert_eq!(
            language_plan(Some("Hinglish / mixed Hindi-English"), false),
            Hinglish
        );
        assert_eq!(
            language_plan(Some("English, Hindi"), false),
            Constrained(vec!["en", "hi"])
        );
        assert_eq!(
            language_plan(Some("en+hi"), false),
            Constrained(vec!["en", "hi"])
        );
        assert_eq!(
            language_plan(Some("English and Marathi"), false),
            Constrained(vec!["en", "mr"])
        );
        // Duplicates collapse; unknown names are skipped.
        assert_eq!(
            language_plan(Some("English, en, Klingon"), false),
            Force("en")
        );
        assert_eq!(language_plan(Some("Klingon"), false), Auto);
        // An English-only model can only decode English.
        assert_eq!(language_plan(Some("English, Hindi"), true), Force("en"));
    }

    #[test]
    fn hinglish_decodes_as_english_but_allows_devanagari() {
        assert_eq!(LanguagePlan::Hinglish.allowed(), Some(vec!["en", "hi"]));
        // An English-only model really is English only.
        assert_eq!(LanguagePlan::Force("en").allowed(), Some(vec!["en"]));
    }

    #[test]
    fn later_segments_switch_language_only_when_clearly_another() {
        let en_hi = ["en", "hi"];
        // Clear Hindi on a long segment: switch.
        assert_eq!(
            keep_or_switch(&[("en", 0.1), ("hi", 0.85)], &en_hi, 5.0, "en"),
            "hi"
        );
        // Accented English scoring "hi" moderately: keep English.
        assert_eq!(
            keep_or_switch(&[("en", 0.35), ("hi", 0.6)], &en_hi, 5.0, "en"),
            "en"
        );
        // Sure, but too short to trust: keep.
        assert_eq!(
            keep_or_switch(&[("en", 0.05), ("hi", 0.9)], &en_hi, 2.0, "en"),
            "en"
        );
        // A language the user doesn't speak never wins.
        assert_eq!(
            keep_or_switch(&[("ar", 0.95), ("hi", 0.02)], &en_hi, 5.0, "en"),
            "en"
        );
        // Same language: nothing to switch.
        assert_eq!(keep_or_switch(&[("en", 0.9)], &en_hi, 5.0, "en"), "en");
    }

    #[test]
    fn prompt_echo_is_dropped_only_when_it_is_one() {
        let dict = Some("Vox, CI/CD");
        let title = Some("List of Cashless Network Garages, ICICI Lombard");
        let prompts = [dict, title];
        // Whisper reciting the title (two parts): dropped even if confident.
        assert!(is_prompt_echo(
            "List of Cashless Network Garages, ICICI Lombard.",
            &prompts,
            Some(0.9),
            false
        ));
        // Saying the site's name clearly into its search box is dictation.
        assert!(!is_prompt_echo("ICICI Lombard", &prompts, Some(0.9), false));
        assert!(!is_prompt_echo("Vox.", &prompts, Some(0.85), false));
        // The same words on a mumble, or when Whisper was guessing: echo.
        assert!(is_prompt_echo("ICICI Lombard", &prompts, Some(0.9), true));
        assert!(is_prompt_echo("ICICI Lombard", &prompts, Some(0.3), false));
        // Part of a part, or a sentence using the words: dictation.
        assert!(!is_prompt_echo(
            "Network Garages",
            &prompts,
            Some(0.3),
            false
        ));
        assert!(!is_prompt_echo(
            "Open the ICICI Lombard claim form",
            &prompts,
            Some(0.3),
            false
        ));
        assert!(!is_prompt_echo("", &prompts, None, true));
        assert!(!is_prompt_echo(
            "ICICI Lombard",
            &[None, None],
            Some(0.1),
            true
        ));
    }

    #[test]
    fn quiet_means_no_window_reaches_speech_level() {
        assert!(is_quiet(&vec![0.003; 16_000]));
        assert!(!is_quiet(&vec![0.1; 16_000]));
    }

    #[test]
    fn constrained_pick_ignores_languages_the_user_does_not_speak() {
        // A 1.9 s clip of accented English that Whisper thought was Arabic.
        let ranked = [("ar", 0.41), ("en", 0.30), ("hi", 0.20), ("pt", 0.09)];
        assert_eq!(
            pick_language(&ranked, Some(&["en", "hi"]), 1.9, None),
            Some("en")
        );
        let ranked = [("ar", 0.2), ("en", 0.1), ("hi", 0.6)];
        assert_eq!(
            pick_language(&ranked, Some(&["en", "hi"]), 5.0, None),
            Some("hi")
        );
        // None of the allowed languages scored at all: the first one.
        assert_eq!(
            pick_language(&[("ar", 1.0)], Some(&["en", "hi"]), 2.0, None),
            Some("en")
        );
    }

    #[test]
    fn auto_pick_falls_back_to_sticky_language_only_when_unsure() {
        let unsure = [("pt", 0.35), ("en", 0.30), ("es", 0.2)];
        // Short and unsure: the language the user was speaking before.
        assert_eq!(pick_language(&unsure, None, 1.9, Some("en")), Some("en"));
        // Long enough to trust the detector, or nothing to fall back to.
        assert_eq!(pick_language(&unsure, None, 6.0, Some("en")), Some("pt"));
        assert_eq!(pick_language(&unsure, None, 1.9, None), Some("pt"));
        // Short but sure.
        let sure = [("hi", 0.9), ("en", 0.1)];
        assert_eq!(pick_language(&sure, None, 1.5, Some("en")), Some("hi"));
        assert_eq!(pick_language(&[], None, 3.0, Some("en")), None);

        assert!(is_confident(4.0, 0.8));
        assert!(!is_confident(2.0, 0.95));
        assert!(!is_confident(10.0, 0.5));
    }

    #[test]
    fn title_hint_drops_version_numbers_and_page_labels() {
        // The title that made Whisper paste "1.146.0" for a whole sentence.
        assert_eq!(
            title_vocabulary(
                Some("Release Notes: 1.146.0 - vox - Visual Studio Code"),
                "Code"
            ),
            Some("vox".into())
        );
        assert_eq!(
            title_vocabulary(
                Some("Extension: Rust Analyzer - Visual Studio Code"),
                "Code"
            ),
            Some("Rust Analyzer".into())
        );
        assert_eq!(
            title_vocabulary(Some("Settings - Visual Studio Code"), "Code"),
            None
        );
        assert_eq!(
            title_vocabulary(
                Some("Release v2.3.1 · Siddharth189/Vox - Google Chrome"),
                "chrome"
            ),
            Some("Release Siddharth189/Vox".into())
        );
        // Counts and issue numbers go; the words around them stay.
        assert_eq!(
            title_vocabulary(Some("PR #684 review - GitHub - Google Chrome"), "chrome"),
            Some("PR review, GitHub".into())
        );
    }

    #[test]
    fn far_too_short_output_is_retried() {
        // Measured: 7.3 s, 9.5 s and 12.3 s of speech.
        assert!(too_short_for("1.", 7.3));
        assert!(too_short_for("1.146.0", 9.5));
        assert!(too_short_for("English", 12.3));
        // Real transcripts of the same lengths are nowhere near the bar.
        assert!(!too_short_for("Hello, my check-up is done.", 4.2));
        assert!(!too_short_for(
            "Check the transcript for some history of the last few days.",
            10.7
        ));
        assert!(
            !too_short_for("सांसों की माला", 3.5),
            "short clips aren't checked"
        );
        assert!(
            !too_short_for("今天我们发布新版本", 4.0),
            "Chinese is dense"
        );
        // A short answer to a short clip is fine.
        assert!(!too_short_for("Yes.", 2.0));
        assert_eq!(content_len("1.146.0"), 5);
    }

    #[test]
    fn title_vocabulary_keeps_names_and_drops_app_noise() {
        assert_eq!(
            title_vocabulary(
                Some("List of Cashless Network Garages - ICICI Lombard - Google Chrome"),
                "chrome"
            ),
            Some("List of Cashless Network Garages, ICICI Lombard".into())
        );
        assert_eq!(
            title_vocabulary(
                Some("(61) Sajan Sajan remix | by Abhishek Mehra - YouTube - Google Chrome"),
                "chrome"
            ),
            Some("Sajan Sajan remix, by Abhishek Mehra, YouTube".into())
        );
        assert_eq!(
            title_vocabulary(Some("Welcome - vox - Visual Studio Code"), "Code"),
            Some("vox".into())
        );
        assert_eq!(
            title_vocabulary(Some("New Tab - Google Chrome"), "chrome"),
            None
        );
        assert_eq!(title_vocabulary(Some("Visual Studio Code"), "Code"), None);
        assert_eq!(title_vocabulary(None, "chrome"), None);
        // Long titles are cut at a part boundary, never mid-word.
        let long = "A very long document title that goes on and on - Second part of it - Third part that cannot fit";
        let hint = title_vocabulary(Some(long), "Word").unwrap();
        assert!(hint.len() <= MAX_TITLE_HINT_LEN);
        assert!(hint.ends_with("on and on, Second part of it"));
        // A part too long to fit is skipped, not the site name after it.
        let page = "An extremely long article headline about quarterly results that exceeds eighty characters - ICICI Lombard - Google Chrome";
        assert_eq!(
            title_vocabulary(Some(page), "chrome"),
            Some("ICICI Lombard".into())
        );
    }

    #[test]
    fn thread_default_is_sane() {
        let t = default_threads();
        assert!((1..=8).contains(&t));
    }

    #[test]
    fn audio_ctx_scales_with_clip_and_has_floor() {
        // 0.5 s clip -> floor
        assert_eq!(audio_ctx_for(8_000), Some(MIN_AUDIO_CTX));
        // 10 s clip -> 500 frames + margin, rounded up to 64
        assert_eq!(audio_ctx_for(160_000), Some(640));
        // 30 s clip -> full context
        assert_eq!(audio_ctx_for(480_000), None);
    }

    #[test]
    fn collapses_decoder_loops_only() {
        assert_eq!(collapse_repeated_sentences("Yes. Yes. Yes. Yes."), "Yes.");
        assert_eq!(collapse_repeated_sentences("Yes. Yes."), "Yes. Yes.");
        // Measured: a real 4.8 s dictation came back like this.
        assert_eq!(
            collapse_repeated_sentences(
                "Should I run it now since PR is a Dane? What should I do? Should I merge it?                  Should I run it now since PR is a Dane? What should I do? Should I merge it?                  Should I run it now since PR is a Dane? What should I do? Should I merge it?                  Should I"
            ),
            "Should I run it now since PR is a Dane? What should I do? Should I merge it?"
        );
        assert_eq!(
            collapse_repeated_sentences(
                "We ship today. Then we rest. We ship today. Then we rest."
            ),
            "We ship today. Then we rest."
        );
        // Too short to call a loop, and not repeated as a whole group.
        assert_eq!(
            collapse_repeated_sentences("Thanks! Bye. Thanks! Bye."),
            "Thanks! Bye. Thanks! Bye."
        );
        assert_eq!(
            collapse_repeated_sentences("We ship today. Then we rest. We ship today."),
            "We ship today. Then we rest. We ship today."
        );
        assert_eq!(
            collapse_repeated_sentences("Ship it. Thanks! Thanks! Thanks! Bye"),
            "Ship it. Thanks! Bye"
        );
        assert_eq!(
            collapse_repeated_sentences("Version 2.5 is out. Great."),
            "Version 2.5 is out. Great."
        );
    }
}
