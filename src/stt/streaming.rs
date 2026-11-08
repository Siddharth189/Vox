//! Transcribing while the user is still talking.
//!
//! Whole-clip transcription makes the wait after the hotkey grow with the
//! dictation: ~1 s for a short reply, ~9 s for a 40 s paragraph on a 4-core
//! laptop. Instead, a worker thread watches the live capture, cuts it at
//! natural pauses into segments, and transcribes each one as soon as it's
//! complete. When the user stops, only the audio since the last kept
//! segment is left to transcribe, so the wait stays roughly constant no
//! matter how long they spoke.
//!
//! Accuracy safeguards, since each segment is decoded on its own:
//! - cuts land in silence (a pause of [`PAUSE_MS`]), never mid-word unless
//!   someone talks for [`HARD_MAX_SECS`] without a breath;
//! - segments are at least [`MIN_SEGMENT_SECS`] long, enough context for
//!   Whisper to get the language and casing right;
//! - a pause isn't always the end of a thought ("ask not ... what your
//!   country can do for you"), and a fragment decoded on its own gets
//!   "fixed" by Whisper ("asked not"). So a segment is only kept when
//!   Whisper ends it as a sentence; otherwise its audio stays pending and is
//!   decoded again, longer, at the next pause. Past [`SOFT_MAX_SECS`] it's
//!   kept regardless. The redo happens while the user is still talking;
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

/// Don't cut before this much audio: shorter segments lose context, make
/// Whisper guess at casing and punctuation, and are more often fragments.
pub const MIN_SEGMENT_SECS: f32 = 6.0;
/// Quiet stretch that counts as the end of a sentence. Gaps between words
/// run ~50-250 ms and comma pauses ~250-650 ms (measured: 460-560 ms in
/// read speech, 660 ms in JFK's inaugural); sentence ends run 800 ms and up.
/// Cutting at a comma pause makes Whisper end the piece with a full stop
/// ("signed. So people..."), so only longer pauses qualify.
pub const PAUSE_MS: u32 = 700;
/// Past this, take the longest pause there is (down to [`SHORT_PAUSE_MS`]),
/// and keep the segment even if it doesn't end a sentence: a long,
/// unbroken segment costs more at the end than an early cut costs in
/// accuracy.
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
    /// Window of the last deferred cut: only pauses that start after it
    /// are candidates, so a long pause isn't re-tried on every poll.
    not_before: Option<usize>,
}

impl Segmenter {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            rate: sample_rate.max(1),
            window: audio::window_len(sample_rate).max(1),
            pending: Vec::new(),
            rms: Vec::new(),
            not_before: None,
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

    /// Where the audio so far could be cut (a window index), if anywhere.
    pub fn next_cut(&self) -> Option<usize> {
        self.find_cut()
    }

    /// The raw samples (native rate) before `cut`.
    pub fn segment(&self, cut: usize) -> &[f32] {
        &self.pending[..(cut * self.window).min(self.pending.len())]
    }

    /// Length of the segment before `cut`.
    pub fn secs_at(&self, cut: usize) -> f32 {
        self.segment(cut).len() as f32 / self.rate as f32
    }

    /// Keep the segment before `cut`: drop it from the pending audio.
    pub fn commit(&mut self, cut: usize) -> Vec<f32> {
        self.not_before = None;
        self.split_at_window(cut)
    }

    /// Don't cut at `cut` after all; look for a later pause instead.
    pub fn defer(&mut self, cut: usize) {
        self.not_before = Some(cut);
    }

    /// Cut and keep the next segment, if there's a place to cut.
    pub fn take_segment(&mut self) -> Option<Vec<f32>> {
        let cut = self.find_cut()?;
        Some(self.commit(cut))
    }

    /// Everything not yet cut (the end of the dictation).
    pub fn take_rest(&mut self) -> Vec<f32> {
        self.rms.clear();
        self.not_before = None;
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

        // Earliest sentence-length pause whose middle falls past the
        // minimum length. Runs are scanned in order; a run still going at
        // the end of the buffer counts once it's long enough (the speaker
        // has paused already). Past the soft max, the longest shorter
        // pause is the fallback.
        let mut longest: Option<(usize, usize)> = None; // (len, mid)
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
            if self.not_before.is_some_and(|nb| start <= nb) {
                continue; // the pause we already tried, or one before it
            }
            let len = i - start;
            let mid = start + len / 2;
            if mid < min {
                continue;
            }
            if len >= pause {
                return Some(mid.min(hard_max));
            }
            if len >= short_pause && longest.is_none_or(|(l, _)| len > l) {
                longest = Some((len, mid));
            }
        }
        if n >= soft_max {
            if let Some((_, mid)) = longest {
                return Some(mid.min(hard_max));
            }
        }

        if n >= hard_max {
            // No pause in 20 s: cut at the quietest window near the cap.
            let from = hard_max
                .saturating_sub(self.windows_for_secs(HARD_CUT_SEARCH_SECS))
                .max(self.not_before.map_or(0, |nb| nb + 1))
                .min(hard_max - 1);
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

/// True when Whisper ended `text` as a sentence (closing quotes and
/// brackets aside), in any script it writes.
pub fn ends_sentence(text: &str) -> bool {
    text.trim_end()
        .trim_end_matches(['"', '\'', ')', ']', '»', '”', '’', '」', '』'])
        .ends_with(['.', '?', '!', '…', '。', '？', '！', '।', '؟', '\u{06D4}'])
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
    /// Segments kept (including the final one).
    pub segments: u32,
    /// Of those, how many were done before the user stopped.
    pub segments_while_speaking: u32,
    /// Streaming failed partway and the whole recording was transcribed at
    /// the end instead.
    pub fell_back: bool,
    /// Speech actually transcribed (after silence trimming), in ms.
    pub audio_ms: u64,
    /// Time spent inside the transcriber across all attempts, in ms: the
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// Done with this audio (kept, or nothing in it worth keeping).
    Keep,
    /// Ends mid-sentence: leave it pending and retry at a later pause.
    Defer,
}

struct Progress {
    texts: Vec<String>,
    language: Option<String>,
    segments: u32,
    failed: Option<VoxError>,
    audio_secs: f64,
    work: Duration,
    /// Sum of segment confidence x segment seconds, and those seconds.
    confidence_weighted: f64,
    confidence_secs: f64,
    /// Why the first discarded segment was discarded.
    rejected: Option<String>,
}

impl Progress {
    fn new() -> Self {
        Self {
            texts: Vec::new(),
            language: None,
            segments: 0,
            failed: None,
            audio_secs: 0.0,
            work: Duration::ZERO,
            confidence_weighted: 0.0,
            confidence_secs: 0.0,
            rejected: None,
        }
    }

    /// Confidence over the whole dictation, weighted by segment length.
    fn confidence(&self) -> Option<f32> {
        (self.confidence_secs > 0.0)
            .then(|| (self.confidence_weighted / self.confidence_secs) as f32)
    }

    fn hint(&self) -> SegmentHint {
        SegmentHint {
            previous_text: tail_chars(&self.texts.join(" "), MAX_CONTEXT_CHARS),
            language: self.language.clone(),
        }
    }

    /// Transcribes `raw`. Unless `must_keep`, a result that doesn't end a
    /// sentence is discarded and `Defer` returned.
    fn transcribe(
        &mut self,
        raw: &[f32],
        rate: u32,
        must_keep: bool,
        transcribe: &mut SegmentFn,
    ) -> Verdict {
        if self.failed.is_some() {
            return Verdict::Keep;
        }
        let chunk = audio::finish_capture(raw, rate);
        // Silent stretches (a long think between sentences) never reach
        // Whisper, which would only hallucinate "Thank you." into them.
        if chunk.samples.is_empty() || audio::is_silent(&chunk.samples, chunk.sample_rate) {
            return Verdict::Keep;
        }
        let started = Instant::now();
        let result = transcribe(&chunk, &self.hint());
        self.work += started.elapsed();
        match result {
            Ok(t) => {
                let text = t.text.trim();
                let speech = !super::is_non_speech(text);
                // A segment discarded as the wrong script is retried with
                // the next phrase too, like a fragment: more audio is what
                // language detection needs. Only at the end is it given up.
                if !must_keep && (t.rejected.is_some() || (speech && !ends_sentence(text))) {
                    return Verdict::Defer;
                }
                self.segments += 1;
                self.audio_secs += chunk.duration_secs();
                if let Some(c) = t.confidence {
                    self.confidence_weighted += c as f64 * chunk.duration_secs();
                    self.confidence_secs += chunk.duration_secs();
                }
                if self.rejected.is_none() {
                    self.rejected = t.rejected.clone();
                }
                // The latest kept segment's language carries on: after a
                // switch (English to Hindi) the next segment starts there.
                if t.rejected.is_none() && t.language.is_some() {
                    self.language = t.language.clone();
                }
                let text = strip_echo(text, &self.texts);
                if speech && !text.is_empty() {
                    self.texts.push(text.to_string());
                }
                Verdict::Keep
            }
            Err(e) => {
                tracing::warn!("streaming segment failed, will transcribe the whole clip: {e}");
                self.failed = Some(e);
                Verdict::Keep
            }
        }
    }

    /// Tries the next cut, if any: keeps it, or defers it to a later pause.
    /// Returns false when there's nothing to cut yet.
    fn advance(&mut self, segmenter: &mut Segmenter, rate: u32, f: &mut SegmentFn) -> bool {
        let Some(cut) = segmenter.next_cut() else {
            return false;
        };
        let must_keep = segmenter.secs_at(cut) >= SOFT_MAX_SECS;
        match self.transcribe(segmenter.segment(cut), rate, must_keep, f) {
            Verdict::Keep => {
                segmenter.commit(cut);
            }
            Verdict::Defer => segmenter.defer(cut),
        }
        true
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
    let mut progress = Progress::new();

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
        while progress.failed.is_none()
            && progress.advance(&mut segmenter, rate, transcribe.as_mut())
        {
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
    // Normally the tail goes to Whisper in one piece: one pass is the most
    // accurate, and splitting it now would only add work after stop. Only
    // a backlog too long for one pass (the worker fell behind) is cut.
    while progress.failed.is_none() && segmenter.pending_secs() > HARD_MAX_SECS {
        let Some(cut) = segmenter.next_cut() else {
            break;
        };
        let segment = segmenter.commit(cut);
        progress.transcribe(&segment, rate, true, transcribe.as_mut());
    }
    let rest = segmenter.take_rest();
    progress.transcribe(&rest, rate, true, transcribe.as_mut());

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
            confidence: progress.confidence(),
            language: progress.language,
            rejected: progress.rejected,
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

/// Prompting with prior text has one known failure mode: Whisper repeats
/// the end of its prompt before carrying on (measured on a 40 s paragraph:
/// "...into the background. Second, we move the speech model download into
/// the background. With progress..."). Drops a start of `text` that is the
/// same 3+ words the previous text ended with; "" if that's all it was.
/// Fewer than 3 words is left alone: "Yes. Yes." is plausible dictation.
fn strip_echo<'a>(text: &'a str, previous: &[String]) -> &'a str {
    const MIN_ECHO_WORDS: usize = 3;
    let norm = |w: &str| -> String {
        w.chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect()
    };
    let before = previous.join(" ");
    let prev: Vec<String> = before
        .split_whitespace()
        .map(norm)
        .filter(|w| !w.is_empty())
        .collect();
    // (byte offset, normalized word) for each word of `text`.
    let words: Vec<(usize, String)> = text
        .split_whitespace()
        .map(|w| (w.as_ptr() as usize - text.as_ptr() as usize, norm(w)))
        .filter(|(_, w)| !w.is_empty())
        .collect();
    let longest = prev.len().min(words.len());
    for k in (MIN_ECHO_WORDS..=longest).rev() {
        let echoed = prev[prev.len() - k..]
            .iter()
            .zip(&words[..k])
            .all(|(a, (_, b))| a == b);
        if echoed {
            return match words.get(k) {
                Some((offset, _)) => &text[*offset..],
                None => "",
            };
        }
    }
    text
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

    fn concat(parts: &[Vec<f32>]) -> Vec<f32> {
        parts.concat()
    }

    #[test]
    fn short_dictation_is_never_cut() {
        let mut s = Segmenter::new(RATE);
        s.push(&tone(2.5));
        s.push(&quiet(0.6));
        s.push(&tone(2.5));
        assert!(s.take_segment().is_none());
        assert!((secs(&s.take_rest()) - 5.6).abs() < 0.01);
    }

    #[test]
    fn cuts_in_the_middle_of_the_first_pause_past_the_minimum() {
        let mut s = Segmenter::new(RATE);
        s.push(&tone(3.0));
        s.push(&quiet(0.8)); // too early: ends before 6 s
        s.push(&tone(3.0)); // 6.8 s
        s.push(&quiet(0.8)); // pause centered at 7.2 s
        s.push(&tone(1.0));
        let seg = s.take_segment().expect("should cut at the second pause");
        assert!((secs(&seg) - 7.2).abs() < 0.03, "cut at {}", secs(&seg));
        assert!(s.take_segment().is_none());
        assert!((secs(&s.take_rest()) - 1.4).abs() < 0.03);
    }

    #[test]
    fn comma_length_pauses_are_not_cut_at() {
        let mut s = Segmenter::new(RATE);
        s.push(&concat(&[
            tone(6.5),
            quiet(0.5),
            tone(3.0),
            quiet(0.55),
            tone(1.0),
        ]));
        assert!(
            s.take_segment().is_none(),
            "500 ms is a comma, not a full stop"
        );
    }

    #[test]
    fn breath_between_words_is_not_a_pause() {
        let mut s = Segmenter::new(RATE);
        for _ in 0..5 {
            s.push(&tone(1.4));
            s.push(&quiet(0.15));
        }
        assert!(s.take_segment().is_none());
    }

    #[test]
    fn a_pause_still_in_progress_counts() {
        let mut s = Segmenter::new(RATE);
        s.push(&tone(7.0));
        s.push(&quiet(0.5));
        assert!(s.take_segment().is_none(), "500 ms isn't a pause yet");
        s.push(&quiet(0.3));
        let seg = s.take_segment().expect("800 ms of quiet is");
        assert!(secs(&seg) > 7.0 && secs(&seg) < 7.5);
    }

    #[test]
    fn past_the_soft_max_the_longest_short_pause_wins() {
        let mut s = Segmenter::new(RATE);
        // 250 ms at 7 s, 450 ms at 10 s, 250 ms at 12.5 s; none sentence-length.
        s.push(&concat(&[
            tone(7.0),
            quiet(0.25),
            tone(2.75),
            quiet(0.45),
            tone(2.3),
            quiet(0.25),
            tone(1.0),
        ]));
        let seg = s.take_segment().expect("past 12 s something must give");
        assert!((secs(&seg) - 10.22).abs() < 0.05, "cut at {}", secs(&seg));
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
    fn deferred_cut_moves_on_to_the_next_pause() {
        let mut s = Segmenter::new(RATE);
        s.push(&concat(&[tone(6.5), quiet(0.8)]));
        let first = s.next_cut().expect("pause after 6.5 s");
        s.defer(first);
        // The same pause, still going, is not offered again...
        s.push(&quiet(1.0));
        assert_eq!(s.next_cut(), None);
        // ...but the next one is, and the segment spans both.
        s.push(&concat(&[tone(2.0), quiet(0.8)]));
        let second = s.next_cut().expect("next pause");
        assert!(second > first);
        assert!(
            (s.secs_at(second) - 10.7).abs() < 0.05,
            "{}",
            s.secs_at(second)
        );
        let seg = s.commit(second);
        assert!((secs(&seg) - 10.7).abs() < 0.05);
        // Committing clears the deferral for what follows.
        s.push(&concat(&[tone(6.5), quiet(0.8)]));
        assert!(s.next_cut().is_some());
    }

    #[test]
    fn push_in_odd_sized_chunks_matches_one_push() {
        let audio = concat(&[tone(3.0), quiet(0.2), tone(3.5), quiet(0.9), tone(1.0)]);
        let mut whole = Segmenter::new(RATE);
        whole.push(&audio);
        let mut pieces = Segmenter::new(RATE);
        for chunk in audio.chunks(777) {
            pieces.push(chunk);
        }
        assert_eq!(whole.rms.len(), pieces.rms.len());
        let cut = whole.take_segment().map(|s| s.len());
        assert!(cut.is_some());
        assert_eq!(cut, pieces.take_segment().map(|s| s.len()));
    }

    #[test]
    fn works_at_native_mic_rates() {
        let rate = 48_000;
        let mut s = Segmenter::new(rate);
        let n = |secs: f32| (rate as f32 * secs) as usize;
        s.push(&vec![0.2; n(7.0)]);
        s.push(&vec![0.0; n(0.8)]);
        s.push(&vec![0.2; n(1.0)]);
        let seg = s.take_segment().expect("cut");
        assert!((seg.len() as f32 / rate as f32 - 7.4).abs() < 0.03);
    }

    #[test]
    fn sentence_endings_in_several_scripts() {
        assert!(ends_sentence("We ship on Friday."));
        assert!(ends_sentence("Really?  "));
        assert!(ends_sentence("He said \"stop.\""));
        assert!(ends_sentence("明日出荷します。"));
        assert!(ends_sentence("हम कल चलेंगे।"));
        assert!(!ends_sentence("And so my fellow Americans, ask not"));
        assert!(!ends_sentence("what your country can do for you,"));
        assert!(!ends_sentence(""));
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
        assert_eq!(strip_echo("ship it on Friday after the review", &prev), "");
        assert_eq!(
            strip_echo("Then we celebrate.", &prev),
            "Then we celebrate."
        );
        assert_eq!(
            strip_echo("the review", &prev),
            "the review",
            "too short to judge"
        );
    }

    #[test]
    fn echoed_start_is_trimmed_and_the_rest_kept() {
        // Measured with real Whisper on a 40 s paragraph.
        let prev = vec![
            "So people stop seeing the smart screen warning.".to_string(),
            "Second, we move the speech model download into the background.".to_string(),
        ];
        assert_eq!(
            strip_echo(
                "Second, we move the speech model download into the background. \
                 With progress shown in the tray menu.",
                &prev
            ),
            "With progress shown in the tray menu."
        );
        // Shared words that aren't at the very end of the previous text stay.
        assert_eq!(
            strip_echo("The smart screen warning is gone now.", &prev),
            "The smart screen warning is gone now."
        );
    }

    type Calls = Arc<Mutex<Vec<(f32, SegmentHint)>>>;

    /// Records each call. Returns `script[n]` for call n when given, else
    /// "Segment number N."; fails on call `fail_on`.
    #[derive(Clone)]
    struct Fake {
        calls: Calls,
        fail_on: Option<usize>,
        script: Vec<&'static str>,
    }

    impl Fake {
        fn new(fail_on: Option<usize>) -> Self {
            Self {
                calls: Arc::new(Mutex::new(Vec::new())),
                fail_on,
                script: Vec::new(),
            }
        }

        fn scripted(script: Vec<&'static str>) -> Self {
            Self {
                script,
                ..Self::new(None)
            }
        }

        fn func(&self) -> impl FnMut(&AudioChunk, &SegmentHint) -> Result<Transcript> + Send {
            let calls = Arc::clone(&self.calls);
            let fail_on = self.fail_on;
            let script = self.script.clone();
            move |chunk, hint| {
                let mut calls = calls.lock().unwrap();
                calls.push((chunk.duration_secs() as f32, hint.clone()));
                let n = calls.len();
                if Some(n) == fail_on {
                    return Err(VoxError::Transcription("boom".into()));
                }
                let text = script
                    .get(n - 1)
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| format!("Segment number {n}."));
                Ok(Transcript {
                    text,
                    language: Some("en".into()),
                    ..Transcript::default()
                })
            }
        }
    }

    fn two_sentences_and_a_tail() -> Vec<f32> {
        concat(&[tone(6.5), quiet(0.8), tone(6.5), quiet(0.8), tone(1.0)])
    }

    fn wait_for_calls(calls: &Calls, n: usize) {
        // Generous deadline for loaded CI machines; normally ~one poll.
        let deadline = Instant::now() + Duration::from_secs(10);
        while calls.lock().unwrap().len() < n && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn session_transcribes_while_speaking_and_joins_in_order() {
        let fake = Fake::new(None);
        let tap = LiveTap::new(RATE);
        let session = StreamingSession::start(tap.clone(), fake.func());
        for chunk in two_sentences_and_a_tail().chunks(RATE as usize / 2) {
            tap.push(chunk);
        }
        wait_for_calls(&fake.calls, 2);
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
            out.audio_ms > 13_000,
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
    fn a_fragment_is_redone_together_with_what_follows() {
        // The JFK case: a dramatic pause mid-sentence. The first attempt
        // doesn't end a sentence, so it's dropped and its audio is decoded
        // again with the next phrase.
        let fake = Fake::scripted(vec![
            "And so my fellow Americans, ask not",
            "And so my fellow Americans, ask not what your country can do for you.",
            "Ask what you can do for your country.",
        ]);
        let tap = LiveTap::new(RATE);
        let session = StreamingSession::start(tap.clone(), fake.func());
        for chunk in two_sentences_and_a_tail().chunks(RATE as usize / 2) {
            tap.push(chunk);
        }
        wait_for_calls(&fake.calls, 2);
        std::thread::sleep(POLL * 2);
        let out = session.finish().unwrap();

        assert_eq!(
            out.transcript.text,
            "And so my fellow Americans, ask not what your country can do for you. \
             Ask what you can do for your country."
        );
        assert_eq!(out.segments, 2, "the dropped attempt doesn't count");
        assert_eq!(out.segments_while_speaking, 1);
        let calls = fake.calls.lock().unwrap();
        assert_eq!(calls.len(), 3);
        assert!(calls[1].0 > calls[0].0 + 6.0, "retry covers both phrases");
        assert_eq!(calls[1].1, SegmentHint::default(), "nothing kept yet");
        // Speech is counted once, and the work of the dropped attempt too.
        assert!((out.audio_ms as f32 / 1000.0 - (calls[1].0 + calls[2].0)).abs() < 0.01);
    }

    #[test]
    fn a_rejected_segment_is_retried_and_reported_if_still_lost() {
        // Segment 1 comes back as the wrong script (rejected): it's retried
        // with the next phrase and decodes fine the second time.
        let calls: Calls = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&calls);
        let mut n = 0;
        let f = move |chunk: &AudioChunk, hint: &SegmentHint| {
            log.lock()
                .unwrap()
                .push((chunk.duration_secs() as f32, hint.clone()));
            n += 1;
            Ok(match n {
                1 => Transcript {
                    rejected: Some("Arabic script".into()),
                    ..Transcript::default()
                },
                2 => Transcript::new("Both phrases, decoded the second time."),
                _ => Transcript::new("The tail."),
            })
        };
        let tap = LiveTap::new(RATE);
        let session = StreamingSession::start(tap.clone(), f);
        for chunk in two_sentences_and_a_tail().chunks(RATE as usize / 2) {
            tap.push(chunk);
        }
        wait_for_calls(&calls, 2);
        std::thread::sleep(POLL * 2);
        let out = session.finish().unwrap();
        assert_eq!(
            out.transcript.text,
            "Both phrases, decoded the second time. The tail."
        );
        assert_eq!(out.transcript.rejected, None, "nothing was lost");
        let calls = calls.lock().unwrap();
        assert!(calls[1].0 > calls[0].0 + 6.0, "retry covers both phrases");

        // Rejected again at the end, where there's no retry: kept text plus
        // a note that part was lost.
        let tap = LiveTap::new(RATE);
        let mut n = 0;
        let f = move |_: &AudioChunk, _: &SegmentHint| {
            n += 1;
            Ok(if n == 1 {
                Transcript::new("First sentence.")
            } else {
                Transcript {
                    rejected: Some("Arabic script".into()),
                    ..Transcript::default()
                }
            })
        };
        tap.push(&two_sentences_and_a_tail());
        let session = StreamingSession::start(tap.clone(), f);
        std::thread::sleep(POLL * 3);
        let out = session.finish().unwrap();
        assert_eq!(out.transcript.text, "First sentence.");
        assert_eq!(out.transcript.rejected.as_deref(), Some("Arabic script"));
    }

    #[test]
    fn unpunctuated_speech_is_still_kept_past_the_soft_max() {
        // A model or language that never punctuates mustn't stop streaming:
        // segments just grow to ~12 s.
        let fake = Fake::scripted(vec!["no punctuation here"; 10]);
        let tap = LiveTap::new(RATE);
        tap.push(&concat(&[
            tone(6.5),
            quiet(0.8),
            tone(6.5),
            quiet(0.8),
            tone(3.0),
        ]));
        let session = StreamingSession::start(tap.clone(), fake.func());
        wait_for_calls(&fake.calls, 2);
        std::thread::sleep(POLL * 2);
        let out = session.finish().unwrap();
        assert_eq!(out.segments_while_speaking, 1, "kept at the 14.2 s pause");
        assert_eq!(out.segments, 2);
    }

    #[test]
    fn the_tail_is_never_deferred() {
        // Second phrase is deferred while speaking (no full stop yet); at
        // stop, everything after the kept first sentence is decoded once
        // and kept even though it doesn't end a sentence either.
        let fake = Fake::scripted(vec!["Done.", "and then we", "and then we stopped"]);
        let tap = LiveTap::new(RATE);
        let session = StreamingSession::start(tap.clone(), fake.func());
        tap.push(&two_sentences_and_a_tail());
        wait_for_calls(&fake.calls, 2);
        let out = session.finish().unwrap();
        assert_eq!(out.transcript.text, "Done. and then we stopped");
        let calls = fake.calls.lock().unwrap();
        assert_eq!(calls.len(), 3);
        assert!(
            calls[2].0 > calls[1].0,
            "the tail includes the deferred phrase"
        );
    }

    #[test]
    fn finish_right_away_transcribes_everything_in_order() {
        let fake = Fake::new(None);
        let tap = LiveTap::new(RATE);
        tap.push(&two_sentences_and_a_tail());
        let out = StreamingSession::start(tap, fake.func()).finish().unwrap();
        assert!(out.segments >= 1);
        assert!(out.transcript.text.starts_with("Segment number 1."));
    }

    #[test]
    fn a_failed_segment_falls_back_to_one_whole_pass() {
        let fake = Fake::new(Some(2));
        let tap = LiveTap::new(RATE);
        let session = StreamingSession::start(tap.clone(), fake.func());
        tap.push(&two_sentences_and_a_tail());
        wait_for_calls(&fake.calls, 2);
        let out = session.finish().unwrap();
        assert!(out.fell_back);
        let calls = fake.calls.lock().unwrap();
        let whole = calls.last().unwrap();
        assert!(whole.0 > 14.0, "fallback covers the whole clip");
        assert_eq!(whole.1, SegmentHint::default());
        assert_eq!(
            out.transcript.text,
            format!("Segment number {}.", calls.len())
        );
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
