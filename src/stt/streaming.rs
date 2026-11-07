//! Transcribing while the user is still talking.
//!
//! Whole-clip transcription makes the wait after the hotkey grow with the
//! dictation: ~1 s for a short reply, ~9 s for a 40 s paragraph on a 4-core
//! laptop. Instead, a worker thread watches the live capture, cuts it at
//! natural pauses into segments of a few seconds, and transcribes each one
//! as soon as it's complete. When the user stops, only the audio since the
//! last pause is left to transcribe, so the wait stays roughly constant no
//! matter how long they spoke.
//!
//! Accuracy safeguards, since each segment is decoded on its own:
//! - cuts land in silence (a pause of [`PAUSE_MS`]), never mid-word unless
//!   someone talks for [`HARD_MAX_SECS`] without a breath;
//! - segments are at least [`MIN_SEGMENT_SECS`] long, enough context for
//!   Whisper to get the language and casing right;
//! - each segment is prompted with the text before it, so it continues the
//!   sentence instead of starting a new one, and in auto-detect mode the
//!   language found in the first segment is kept for the rest;
//! - if any segment fails, the whole recording is transcribed in one go at
//!   the end, exactly as without streaming.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::audio::{self, LiveTap, SILENCE_RMS};
use crate::error::{Result, VoxError};
use crate::model::{AudioChunk, Transcript};

/// Don't cut before this much audio: shorter segments lose context and
/// make Whisper guess at casing and punctuation.
pub const MIN_SEGMENT_SECS: f32 = 4.0;
/// Quiet stretch that counts as a pause between phrases. Within a sentence,
/// gaps between words run ~50-250 ms; between phrases, 300 ms and up.
pub const PAUSE_MS: u32 = 400;
/// Past this, take any shorter pause: a long, breathless segment costs more
/// at the end than a slightly early cut costs in accuracy.
pub const SOFT_MAX_SECS: f32 = 12.0;
pub const SHORT_PAUSE_MS: u32 = 200;
/// Never let a segment grow past this (Whisper's window is 30 s). Cut at the
/// quietest moment in the last [`HARD_CUT_SEARCH_SECS`] instead.
pub const HARD_MAX_SECS: f32 = 20.0;
const HARD_CUT_SEARCH_SECS: f32 = 3.0;
/// Quiet = below this fraction of the segment's loud (95th-percentile)
/// level, the same rule `trim_silence` uses for the edges of a clip.
const QUIET_RATIO: f32 = 0.1;
const PEAK_PERCENTILE: f32 = 0.95;
/// How often the worker looks for new audio.
const POLL: Duration = Duration::from_millis(150);
/// Prior text passed to Whisper as context for the next segment.
const MAX_CONTEXT_CHARS: usize = 200;

/// Splits a growing stream of raw mono samples into speech segments at
/// pauses. Pure and synchronous, so the cut rules are testable without
/// threads, a microphone, or a model.
pub struct Segmenter {
    rate: u32,
    window: usize,
    /// Raw samples since the last cut.
    pending: Vec<f32>,
    /// RMS of each complete 20 ms window of `pending`.
    rms: Vec<f32>,
}

impl Segmenter {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            rate: sample_rate.max(1),
            window: audio::window_len(sample_rate).max(1),
            pending: Vec::new(),
            rms: Vec::new(),
        }
    }

    pub fn push(&mut self, samples: &[f32]) {
        let done = self.rms.len() * self.window;
        self.pending.extend_from_slice(samples);
        let complete = (self.pending.len() - done) / self.window * self.window;
        if complete > 0 {
            let new = audio::window_rms(&self.pending[done..done + complete], self.rate);
            self.rms.extend(new);
        }
    }

    pub fn pending_secs(&self) -> f32 {
        self.pending.len() as f32 / self.rate as f32
    }

    fn windows_for_secs(&self, secs: f32) -> usize {
        (secs * 1000.0 / self.window_ms()).round() as usize
    }

    fn windows_for_ms(&self, ms: u32) -> usize {
        (ms as f32 / self.window_ms()).round().max(1.0) as usize
    }

    fn window_ms(&self) -> f32 {
        self.window as f32 * 1000.0 / self.rate as f32
    }

    /// The next complete segment (raw samples, native rate), if the audio so
    /// far has a good place to cut. Call repeatedly until it returns None.
    pub fn take_segment(&mut self) -> Option<Vec<f32>> {
        let cut = self.find_cut()?;
        Some(self.split_at_window(cut))
    }

    /// Everything not yet cut (the end of the dictation).
    pub fn take_rest(&mut self) -> Vec<f32> {
        self.rms.clear();
        std::mem::take(&mut self.pending)
    }

    fn split_at_window(&mut self, cut: usize) -> Vec<f32> {
        let samples = (cut * self.window).min(self.pending.len());
        let rest = self.pending.split_off(samples);
        self.rms.drain(..cut.min(self.rms.len()));
        std::mem::replace(&mut self.pending, rest)
    }

    /// Window index to cut at, or None to keep listening.
    fn find_cut(&self) -> Option<usize> {
        let n = self.rms.len();
        let min = self.windows_for_secs(MIN_SEGMENT_SECS);
        if n <= min {
            return None;
        }
        let threshold = quiet_threshold(&self.rms);
        let soft_max = self.windows_for_secs(SOFT_MAX_SECS);
        let hard_max = self.windows_for_secs(HARD_MAX_SECS);
        let pause = self.windows_for_ms(PAUSE_MS);
        let short_pause = self.windows_for_ms(SHORT_PAUSE_MS);

        // Earliest pause whose middle falls past the minimum length. Runs
        // are scanned in order; a run still going at the end of the buffer
        // counts once it's long enough (the speaker has paused already).
        let mut i = 0;
        while i < n {
            if self.rms[i] >= threshold {
                i += 1;
                continue;
            }
            let start = i;
            while i < n && self.rms[i] < threshold {
                i += 1;
            }
            let len = i - start;
            let mid = start + len / 2;
            let needed = if mid >= soft_max { short_pause } else { pause };
            if len >= needed && mid >= min {
                return Some(mid.min(hard_max));
            }
        }

        if n >= hard_max {
            // No pause in 20 s: cut at the quietest window near the cap.
            let from = hard_max.saturating_sub(self.windows_for_secs(HARD_CUT_SEARCH_SECS));
            let quietest = (from..hard_max)
                .min_by(|&a, &b| self.rms[a].total_cmp(&self.rms[b]))
                .unwrap_or(hard_max);
            return Some(quietest.max(1));
        }
        None
    }
}

fn quiet_threshold(rms: &[f32]) -> f32 {
    let mut sorted = rms.to_vec();
    sorted.sort_by(f32::total_cmp);
    let rank = ((PEAK_PERCENTILE * sorted.len() as f32).ceil() as usize)
        .saturating_sub(1)
        .min(sorted.len().saturating_sub(1));
    let peak = sorted.get(rank).copied().unwrap_or(0.0);
    (peak * QUIET_RATIO).max(SILENCE_RMS)
}

/// What the transcriber should know about the segments before this one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SegmentHint {
    /// The end of the text transcribed so far, as decoding context.
    pub previous_text: String,
    /// Language detected in an earlier segment, to keep for this one.
    pub language: Option<String>,
}

/// Result of a streamed dictation.
#[derive(Debug, Clone)]
pub struct StreamOutcome {
    pub transcript: Transcript,
    /// Segments transcribed (including the final one).
    pub segments: u32,
    /// Of those, how many were done before the user stopped.
    pub segments_while_speaking: u32,
    /// Streaming failed partway and the whole recording was transcribed at
    /// the end instead.
    pub fell_back: bool,
    /// Speech actually transcribed (after silence trimming), in ms.
    pub audio_ms: u64,
    /// Time spent inside the transcriber across all segments, in ms: the
    /// honest measure of Whisper's speed, unlike the wait after stop.
    pub work_ms: u64,
}

enum Signal {
    Finish,
    Cancel,
}

/// Transcribes one segment. In the app this is the pipeline's Whisper; in
/// tests, a fake.
pub type SegmentFn = dyn FnMut(&AudioChunk, &SegmentHint) -> Result<Transcript> + Send;

/// A background transcription of one dictation in progress. Dropping it
/// without `finish()` cancels: the worker stops after the segment it's on
/// and the text is discarded.
pub struct StreamingSession {
    control: Sender<Signal>,
    handle: Option<JoinHandle<Option<Result<StreamOutcome>>>>,
}

impl StreamingSession {
    pub fn start<F>(tap: LiveTap, transcribe: F) -> Self
    where
        F: FnMut(&AudioChunk, &SegmentHint) -> Result<Transcript> + Send + 'static,
    {
        let (control, rx) = mpsc::channel();
        let handle = thread::Builder::new()
            .name("vox-stream-stt".into())
            .spawn(move || worker(tap, rx, Box::new(transcribe)))
            .ok();
        Self { control, handle }
    }

    /// Call once capture has stopped: transcribes what's left and returns
    /// the whole dictation's text.
    pub fn finish(mut self) -> Result<StreamOutcome> {
        let _ = self.control.send(Signal::Finish);
        let handle = self
            .handle
            .take()
            .ok_or_else(|| VoxError::Transcription("streaming worker didn't start".into()))?;
        match handle.join() {
            Ok(Some(result)) => result,
            Ok(None) => Err(VoxError::Transcription("streaming cancelled".into())),
            Err(_) => Err(VoxError::Transcription("streaming worker panicked".into())),
        }
    }
}

impl Drop for StreamingSession {
    fn drop(&mut self) {
        if self.handle.is_some() {
            let _ = self.control.send(Signal::Cancel);
        }
    }
}

struct Progress {
    texts: Vec<String>,
    language: Option<String>,
    segments: u32,
    failed: Option<VoxError>,
    audio_secs: f64,
    work: Duration,
}

impl Progress {
    fn hint(&self) -> SegmentHint {
        SegmentHint {
            previous_text: tail_chars(&self.texts.join(" "), MAX_CONTEXT_CHARS),
            language: self.language.clone(),
        }
    }

    fn transcribe(&mut self, raw: &[f32], rate: u32, transcribe: &mut SegmentFn) {
        if self.failed.is_some() {
            return;
        }
        let chunk = audio::finish_capture(raw, rate);
        // Silent stretches (a long think between sentences) never reach
        // Whisper, which would only hallucinate "Thank you." into them.
        if chunk.samples.is_empty() || audio::is_silent(&chunk.samples, chunk.sample_rate) {
            return;
        }
        let started = Instant::now();
        let result = transcribe(&chunk, &self.hint());
        self.work += started.elapsed();
        match result {
            Ok(t) => {
                self.segments += 1;
                self.audio_secs += chunk.duration_secs();
                if self.language.is_none() {
                    self.language = t.language.clone();
                }
                let text = t.text.trim();
                if super::is_non_speech(text) || repeats_previous(text, &self.texts) {
                    return;
                }
                self.texts.push(text.to_string());
            }
            Err(e) => {
                tracing::warn!("streaming segment failed, will transcribe the whole clip: {e}");
                self.failed = Some(e);
            }
        }
    }
}

fn worker(
    tap: LiveTap,
    rx: Receiver<Signal>,
    mut transcribe: Box<SegmentFn>,
) -> Option<Result<StreamOutcome>> {
    let rate = tap.sample_rate();
    let mut segmenter = Segmenter::new(rate);
    let mut read_pos = 0usize;
    let mut fresh = Vec::new();
    let mut progress = Progress {
        texts: Vec::new(),
        language: None,
        segments: 0,
        failed: None,
        audio_secs: 0.0,
        work: Duration::ZERO,
    };

    // While recording: transcribe each segment as soon as it's complete.
    loop {
        match rx.recv_timeout(POLL) {
            Ok(Signal::Cancel) | Err(RecvTimeoutError::Disconnected) => return None,
            Ok(Signal::Finish) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }
        fresh.clear();
        read_pos = tap.read_from(read_pos, &mut fresh);
        segmenter.push(&fresh);
        while progress.failed.is_none() {
            let Some(segment) = segmenter.take_segment() else {
                break;
            };
            progress.transcribe(&segment, rate, transcribe.as_mut());
            match rx.try_recv() {
                Ok(Signal::Cancel) | Err(TryRecvError::Disconnected) => return None,
                // Finish mid-backlog: keep going, the rest is the tail.
                Ok(Signal::Finish) => {
                    return Some(finish(tap, segmenter, read_pos, progress, transcribe))
                }
                Err(TryRecvError::Empty) => {}
            }
        }
    }
    Some(finish(tap, segmenter, read_pos, progress, transcribe))
}

/// Capture has stopped: drain the tap, transcribe what's left, join.
fn finish(
    tap: LiveTap,
    mut segmenter: Segmenter,
    read_pos: usize,
    mut progress: Progress,
    mut transcribe: Box<SegmentFn>,
) -> Result<StreamOutcome> {
    let rate = tap.sample_rate();
    let segments_while_speaking = progress.segments;
    let mut fresh = Vec::new();
    tap.read_from(read_pos, &mut fresh);
    segmenter.push(&fresh);
    // Normally just the tail; more if the worker fell behind real time.
    while progress.failed.is_none() {
        let Some(segment) = segmenter.take_segment() else {
            break;
        };
        progress.transcribe(&segment, rate, transcribe.as_mut());
    }
    let rest = segmenter.take_rest();
    progress.transcribe(&rest, rate, transcribe.as_mut());

    if progress.failed.is_some() {
        // Same result as without streaming: one pass over everything.
        let mut all = Vec::new();
        tap.read_from(0, &mut all);
        let chunk = audio::finish_capture(&all, rate);
        if chunk.samples.is_empty() || audio::is_silent(&chunk.samples, chunk.sample_rate) {
            return Ok(StreamOutcome {
                transcript: Transcript::new(""),
                segments: 0,
                segments_while_speaking: 0,
                fell_back: true,
                audio_ms: 0,
                work_ms: 0,
            });
        }
        let started = Instant::now();
        let transcript = transcribe(&chunk, &SegmentHint::default())?;
        return Ok(StreamOutcome {
            transcript,
            segments: 1,
            segments_while_speaking: 0,
            fell_back: true,
            audio_ms: secs_to_ms(chunk.duration_secs()),
            work_ms: crate::model::elapsed_ms(started.elapsed()),
        });
    }

    Ok(StreamOutcome {
        transcript: Transcript {
            text: progress.texts.join(" "),
            language: progress.language,
        },
        segments: progress.segments,
        segments_while_speaking,
        fell_back: false,
        audio_ms: secs_to_ms(progress.audio_secs),
        work_ms: crate::model::elapsed_ms(progress.work),
    })
}

fn secs_to_ms(secs: f64) -> u64 {
    (secs * 1000.0).round() as u64
}

/// The last `max` characters of `text`, starting at a word boundary.
fn tail_chars(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    let start = text
        .char_indices()
        .nth(count - max)
        .map(|(i, _)| i)
        .unwrap_or(0);
    let tail = &text[start..];
    match tail.find(char::is_whitespace) {
        Some(sp) => tail[sp..].trim_start().to_string(),
        None => tail.to_string(),
    }
}

/// Prompting with prior text has one known failure mode: on a quiet or
/// unclear segment Whisper sometimes just echoes the end of its prompt.
/// Drop a segment whose text is nothing but a repeat of what came before.
fn repeats_previous(text: &str, previous: &[String]) -> bool {
    let norm = |s: &str| -> String {
        s.chars()
            .filter(|c| c.is_alphanumeric() || c.is_whitespace())
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    };
    let new = norm(text);
    if new.split(' ').count() < 3 {
        return false; // "Yes. Yes." is plausible dictation
    }
    let before = norm(&previous.join(" "));
    before.ends_with(&new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    const RATE: u32 = 16_000;

    fn tone(secs: f32) -> Vec<f32> {
        let n = (RATE as f32 * secs) as usize;
        (0..n)
            .map(|i| 0.2 * (2.0 * std::f32::consts::PI * 220.0 * i as f32 / RATE as f32).sin())
            .collect()
    }

    fn quiet(secs: f32) -> Vec<f32> {
        vec![0.0005; (RATE as f32 * secs) as usize]
    }

    fn secs(samples: &[f32]) -> f32 {
        samples.len() as f32 / RATE as f32
    }

    #[test]
    fn short_dictation_is_never_cut() {
        let mut s = Segmenter::new(RATE);
        s.push(&tone(1.5));
        s.push(&quiet(0.6));
        s.push(&tone(1.5));
        assert!(s.take_segment().is_none());
        assert!((secs(&s.take_rest()) - 3.6).abs() < 0.01);
    }

    #[test]
    fn cuts_in_the_middle_of_the_first_pause_past_the_minimum() {
        let mut s = Segmenter::new(RATE);
        s.push(&tone(2.0));
        s.push(&quiet(0.6)); // too early: ends before 4 s
        s.push(&tone(2.5)); // 5.1 s
        s.push(&quiet(0.6)); // pause centered at 5.4 s
        s.push(&tone(1.0));
        let seg = s.take_segment().expect("should cut at the second pause");
        assert!((secs(&seg) - 5.4).abs() < 0.03, "cut at {}", secs(&seg));
        assert!(s.take_segment().is_none());
        assert!((secs(&s.take_rest()) - 1.3).abs() < 0.03);
    }

    #[test]
    fn breath_between_words_is_not_a_pause() {
        let mut s = Segmenter::new(RATE);
        for _ in 0..4 {
            s.push(&tone(1.4));
            s.push(&quiet(0.15));
        }
        assert!(s.take_segment().is_none());
    }

    #[test]
    fn a_pause_still_in_progress_counts() {
        let mut s = Segmenter::new(RATE);
        s.push(&tone(5.0));
        s.push(&quiet(0.3));
        assert!(s.take_segment().is_none(), "300 ms isn't a pause yet");
        s.push(&quiet(0.2));
        let seg = s.take_segment().expect("500 ms of quiet is");
        assert!(secs(&seg) > 5.0 && secs(&seg) < 5.5);
    }

    #[test]
    fn long_segments_accept_shorter_pauses() {
        let mut s = Segmenter::new(RATE);
        s.push(&tone(13.0));
        s.push(&quiet(0.25));
        s.push(&tone(1.0));
        let seg = s.take_segment().expect("past 12 s a 250 ms gap is enough");
        assert!((secs(&seg) - 13.12).abs() < 0.05, "cut at {}", secs(&seg));
    }

    #[test]
    fn nonstop_speech_is_cut_at_the_cap_on_the_quietest_moment() {
        let mut s = Segmenter::new(RATE);
        let mut speech = tone(25.0);
        // A softer (not silent) syllable gap at 18.5 s, inside the search band.
        let at = (18.5 * RATE as f32) as usize;
        for v in &mut speech[at..at + 1600] {
            *v *= 0.3;
        }
        s.push(&speech);
        let seg = s.take_segment().expect("must cut by 20 s");
        assert!(secs(&seg) <= HARD_MAX_SECS);
        assert!((secs(&seg) - 18.5).abs() < 0.05, "cut at {}", secs(&seg));
    }

    #[test]
    fn push_in_odd_sized_chunks_matches_one_push() {
        let mut audio = tone(3.0);
        audio.extend(quiet(0.2));
        audio.extend(tone(2.0));
        audio.extend(quiet(0.7));
        audio.extend(tone(1.0));
        let mut whole = Segmenter::new(RATE);
        whole.push(&audio);
        let mut pieces = Segmenter::new(RATE);
        for chunk in audio.chunks(777) {
            pieces.push(chunk);
        }
        assert_eq!(whole.rms.len(), pieces.rms.len());
        assert_eq!(
            whole.take_segment().map(|s| s.len()),
            pieces.take_segment().map(|s| s.len())
        );
    }

    #[test]
    fn works_at_native_mic_rates() {
        let rate = 48_000;
        let mut s = Segmenter::new(rate);
        let n = |secs: f32| (rate as f32 * secs) as usize;
        s.push(&vec![0.2; n(5.0)]);
        s.push(&vec![0.0; n(0.6)]);
        s.push(&vec![0.2; n(1.0)]);
        let seg = s.take_segment().expect("cut");
        assert!((seg.len() as f32 / rate as f32 - 5.3).abs() < 0.03);
    }

    #[test]
    fn tail_keeps_whole_words() {
        assert_eq!(tail_chars("short", 200), "short");
        assert_eq!(tail_chars("alpha beta gamma delta", 11), "delta");
        assert_eq!(tail_chars("héllo wörld ünïcode", 9), "ünïcode");
    }

    #[test]
    fn echo_of_previous_text_is_dropped() {
        let prev = vec!["We should ship it on Friday after the review.".to_string()];
        assert!(repeats_previous(
            "ship it on Friday after the review",
            &prev
        ));
        assert!(!repeats_previous("Then we celebrate.", &prev));
        assert!(!repeats_previous("the review", &prev), "too short to judge");
    }

    /// Records each call; returns "seg N" (or fails on call `fail_on`).
    #[derive(Clone)]
    struct Fake {
        calls: Arc<Mutex<Vec<(f32, SegmentHint)>>>,
        fail_on: Option<usize>,
    }

    impl Fake {
        fn new(fail_on: Option<usize>) -> Self {
            Self {
                calls: Arc::new(Mutex::new(Vec::new())),
                fail_on,
            }
        }

        fn func(&self) -> impl FnMut(&AudioChunk, &SegmentHint) -> Result<Transcript> + Send {
            let calls = Arc::clone(&self.calls);
            let fail_on = self.fail_on;
            move |chunk, hint| {
                let mut calls = calls.lock().unwrap();
                calls.push((chunk.duration_secs() as f32, hint.clone()));
                let n = calls.len();
                if Some(n) == fail_on {
                    return Err(VoxError::Transcription("boom".into()));
                }
                Ok(Transcript {
                    text: format!("Segment number {n}."),
                    language: Some("en".into()),
                })
            }
        }
    }

    fn two_sentences_and_a_tail() -> Vec<f32> {
        let mut a = tone(4.5);
        a.extend(quiet(0.6));
        a.extend(tone(4.5));
        a.extend(quiet(0.6));
        a.extend(tone(1.0));
        a
    }

    #[test]
    fn session_transcribes_while_speaking_and_joins_in_order() {
        let fake = Fake::new(None);
        let tap = LiveTap::new(RATE);
        let session = StreamingSession::start(tap.clone(), fake.func());
        for chunk in two_sentences_and_a_tail().chunks(RATE as usize / 2) {
            tap.push(chunk);
        }
        // Let the worker catch up as it would during real speech (generous
        // deadline for loaded CI machines; normally ~one poll interval).
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while fake.calls.lock().unwrap().len() < 2 && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        // ...and confirm it doesn't jump ahead and cut the unfinished tail.
        std::thread::sleep(POLL * 2);
        let segments_before_stop = fake.calls.lock().unwrap().len();
        let out = session.finish().unwrap();

        assert_eq!(segments_before_stop, 2, "both sentences done before stop");
        assert_eq!(out.segments, 3);
        assert_eq!(out.segments_while_speaking, 2);
        assert!(!out.fell_back);
        let summed: f32 = fake.calls.lock().unwrap().iter().map(|c| c.0).sum();
        assert!((out.audio_ms as f32 / 1000.0 - summed).abs() < 0.01);
        assert!(
            out.audio_ms > 9_000,
            "all speech counted, got {}",
            out.audio_ms
        );
        assert_eq!(
            out.transcript.text,
            "Segment number 1. Segment number 2. Segment number 3."
        );
        let calls = fake.calls.lock().unwrap();
        assert_eq!(calls[0].1, SegmentHint::default());
        assert_eq!(calls[1].1.previous_text, "Segment number 1.");
        assert_eq!(calls[1].1.language.as_deref(), Some("en"));
        assert_eq!(
            calls[2].1.previous_text,
            "Segment number 1. Segment number 2."
        );
        // The tail is short, which is the whole point.
        assert!(calls[2].0 < 2.0, "tail was {} s", calls[2].0);
    }

    #[test]
    fn finish_right_away_transcribes_everything_in_order() {
        let fake = Fake::new(None);
        let tap = LiveTap::new(RATE);
        tap.push(&two_sentences_and_a_tail());
        let out = StreamingSession::start(tap, fake.func()).finish().unwrap();
        assert_eq!(out.segments, 3);
        assert!(out.transcript.text.ends_with("Segment number 3."));
    }

    #[test]
    fn a_failed_segment_falls_back_to_one_whole_pass() {
        let fake = Fake::new(Some(2));
        let tap = LiveTap::new(RATE);
        tap.push(&two_sentences_and_a_tail());
        let out = StreamingSession::start(tap, fake.func()).finish().unwrap();
        assert!(out.fell_back);
        assert_eq!(out.transcript.text, "Segment number 3.");
        let calls = fake.calls.lock().unwrap();
        let whole = calls.last().unwrap();
        assert!(whole.0 > 10.0, "fallback covers the whole clip");
        assert_eq!(whole.1, SegmentHint::default());
    }

    #[test]
    fn silence_never_reaches_the_transcriber() {
        let fake = Fake::new(None);
        let tap = LiveTap::new(RATE);
        tap.push(&quiet(8.0));
        let out = StreamingSession::start(tap, fake.func()).finish().unwrap();
        assert_eq!(out.transcript.text, "");
        assert_eq!(fake.calls.lock().unwrap().len(), 0);
    }

    #[test]
    fn dropping_the_session_cancels_it() {
        let fake = Fake::new(None);
        let tap = LiveTap::new(RATE);
        let session = StreamingSession::start(tap.clone(), fake.func());
        drop(session);
        tap.push(&two_sentences_and_a_tail());
        std::thread::sleep(POLL * 3);
        assert_eq!(fake.calls.lock().unwrap().len(), 0);
    }
}
