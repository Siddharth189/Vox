//! The live text window: a small bubble near the bottom of the screen that
//! shows a level meter and the words as they're spoken, then "Writing..."
//! until the text is pasted.
//!
//! It never takes focus and ignores the mouse, so it can't change where the
//! dictation goes. Windows only for now (see `windows.rs`); elsewhere
//! [`Overlay::new`] returns `None` and the tray shows the text in its title.

#[cfg(windows)]
mod windows;

use crate::audio::LiveTap;
use crate::stt::LiveText;

/// What the bubble is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Recording: the level meter moves with the microphone.
    Listening,
    /// Recording an edit-by-voice instruction.
    ListeningEdit,
    /// Recording stopped; transcribing, polishing, or pasting.
    Working,
    /// A short message (an error, "nothing selected"), hidden after a moment.
    Message,
}

/// The live text window, where the platform has one.
pub struct Overlay {
    #[cfg(windows)]
    inner: windows::Window,
}

impl Overlay {
    /// `None` where there's no overlay (macOS, Linux) or it couldn't be
    /// created; callers carry on without it.
    pub fn new() -> Option<Self> {
        #[cfg(windows)]
        {
            match windows::Window::new() {
                Ok(inner) => Some(Self { inner }),
                Err(e) => {
                    tracing::warn!("live text window unavailable: {e}");
                    None
                }
            }
        }
        #[cfg(not(windows))]
        {
            None
        }
    }

    /// Shows the bubble for a new recording. `tap` drives the level meter.
    pub fn listen(&mut self, title: &str, edit: bool, tap: Option<LiveTap>) {
        let phase = if edit {
            Phase::ListeningEdit
        } else {
            Phase::Listening
        };
        #[cfg(windows)]
        self.inner.show(phase, title, tap);
        #[cfg(not(windows))]
        let _ = (phase, title, tap);
    }

    /// The words so far, while listening or working.
    pub fn set_text(&mut self, text: &LiveText) {
        #[cfg(windows)]
        self.inner.set_text(&text.settled, &text.pending);
        #[cfg(not(windows))]
        let _ = text;
    }

    /// Recording stopped: keep the text, stop the meter.
    pub fn working(&mut self, title: &str) {
        #[cfg(windows)]
        self.inner.set_phase(Phase::Working, title);
        #[cfg(not(windows))]
        let _ = title;
    }

    /// Shows `message` alone for a moment, then hides.
    pub fn flash(&mut self, message: &str) {
        #[cfg(windows)]
        {
            self.inner.set_text("", "");
            self.inner.set_phase(Phase::Message, message);
        }
        #[cfg(not(windows))]
        let _ = message;
    }

    pub fn hide(&mut self) {
        #[cfg(windows)]
        self.inner.hide();
    }
}

/// Where to start showing `text` so that "…" plus the rest fits, as judged
/// by `fits`: a byte offset at the start of a word (0 when it all fits).
/// The oldest words are dropped first; the newest always stay.
pub fn tail_start(text: &str, fits: impl Fn(&str) -> bool) -> usize {
    if fits(text) {
        return 0;
    }
    let starts: Vec<usize> = text
        .char_indices()
        .zip(text.chars().skip(1))
        .filter(|((_, c), next)| c.is_whitespace() && !next.is_whitespace())
        .map(|((i, c), _)| i + c.len_utf8())
        .collect();
    let Some(&last) = starts.last() else {
        return 0;
    };
    // Fewer words always fit at least as well, so the first start that
    // fits can be found by bisection.
    let (mut lo, mut hi) = (0usize, starts.len() - 1);
    if !fits(&ellipsized(text, last)) {
        return last;
    }
    while lo < hi {
        let mid = (lo + hi) / 2;
        if fits(&ellipsized(text, starts[mid])) {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    starts[lo]
}

/// `text` from byte `start` on, with "…" for what was left out.
pub fn ellipsized(text: &str, start: usize) -> String {
    if start == 0 {
        text.to_string()
    } else {
        format!("…{}", &text[start..])
    }
}

/// Level meter input: the loudness of the last `window` samples, mapped
/// from -60..-10 dBFS onto 0..1.
pub fn meter_level(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let mean_sq = samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32;
    let db = 10.0 * mean_sq.max(1e-12).log10();
    ((db + 60.0) / 50.0).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_is_shown_whole() {
        assert_eq!(tail_start("hello there", |_| true), 0);
    }

    #[test]
    fn oldest_words_are_dropped_until_it_fits() {
        let text = "one two three four five";
        let start = tail_start(text, |s| s.chars().count() <= 12);
        assert_eq!(ellipsized(text, start), "…four five");
        // The most that fits, not just something that fits.
        let start = tail_start(text, |s| s.chars().count() <= 16);
        assert_eq!(ellipsized(text, start), "…three four five");
    }

    #[test]
    fn a_single_overlong_word_is_kept() {
        let text = "a supercalifragilistic";
        let start = tail_start(text, |s| s.chars().count() <= 5);
        assert_eq!(ellipsized(text, start), "…supercalifragilistic");
        assert_eq!(tail_start("unbreakable", |_| false), 0);
    }

    #[test]
    fn word_starts_respect_multibyte_text_and_runs_of_spaces() {
        let text = "आज मैं  hospital जाऊँगा";
        let start = tail_start(text, |s| s.chars().count() <= 16);
        assert_eq!(ellipsized(text, start), "…hospital जाऊँगा");
    }

    #[test]
    fn meter_maps_loudness_onto_zero_to_one() {
        assert_eq!(meter_level(&[]), 0.0);
        assert_eq!(meter_level(&[0.0; 160]), 0.0);
        assert_eq!(meter_level(&[1.0; 160]), 1.0);
        let speech = meter_level(&[0.05; 160]); // about -26 dBFS
        assert!(speech > 0.6 && speech < 0.75, "{speech}");
    }
}
