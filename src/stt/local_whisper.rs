use std::path::Path;
use std::sync::Mutex;

use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState,
};

use crate::audio::{self, TARGET_RATE};
use crate::error::{Result, VoxError};
use crate::model::{AppContext, AudioChunk, Transcript};
use crate::stt::Transcriber;

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
        let _ = self.run(&chunk.samples);
    }

    /// The whisper language code to request. whisper.cpp's own default is
    /// "en", so "auto" has to be passed explicitly to actually auto-detect.
    fn language_param(&self) -> Option<&'static str> {
        if self.english_only_model {
            return Some("en");
        }
        let lang = self.input_language.as_deref().unwrap_or("auto");
        let lower = lang.to_ascii_lowercase();
        // Hinglish/code-switched speech decodes best as Latin-script English
        // (what the cleanup prompt's few-shot examples expect); language ID
        // would flip between hi/ur/en per clip.
        if lower.contains("hinglish") || lower.contains("mixed") || lower.contains("code-switch") {
            return Some("en");
        }
        whisper_language_code(lang).or(Some("auto"))
    }

    fn run(&self, samples: &[f32]) -> Result<String> {
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
        params.set_language(self.language_param());
        if let Some(ctx) = audio_ctx_for(samples.len()) {
            params.set_audio_ctx(ctx);
        }
        // Fast speech is ~5 tokens/s; anything far beyond that is the
        // decoder looping, so bound it instead of letting it run to 224.
        let secs = samples.len() as f32 / TARGET_RATE as f32;
        params.set_max_tokens((secs * 10.0).ceil() as i32 + 16);

        if let Some(ref prompt) = self.initial_prompt {
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
        if let Err(e) = result {
            // A failed run can leave the state inconsistent; rebuild next time.
            *guard = None;
            return Err(VoxError::Transcription(e.to_string()));
        }

        let n = state
            .full_n_segments()
            .map_err(|e| VoxError::Transcription(e.to_string()))?;
        let mut text = String::new();
        for i in 0..n {
            let seg = state
                .full_get_segment_text(i)
                .map_err(|e| VoxError::Transcription(e.to_string()))?;
            text.push_str(&seg);
        }
        Ok(collapse_repeated_sentences(text.trim()))
    }
}

/// Safety net for decoder loops: three or more identical sentences in a row
/// ("Yes. Yes. Yes.") are a hallucination, never dictation; keep one.
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

    let mut out: Vec<&str> = Vec::new();
    let mut collapsed = false;
    let mut i = 0;
    while i < sentences.len() {
        let mut j = i + 1;
        while j < sentences.len() && sentences[j].eq_ignore_ascii_case(sentences[i]) {
            j += 1;
        }
        let run = j - i;
        let keep = if run >= 3 { 1 } else { run };
        collapsed |= keep != run;
        out.extend(std::iter::repeat_n(sentences[i], keep));
        i = j;
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
    fn transcribe(&self, audio: &AudioChunk, _ctx: &AppContext) -> Result<Transcript> {
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
        Ok(Transcript::new(self.run(&audio.samples)?))
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
