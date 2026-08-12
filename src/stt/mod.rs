mod local_whisper;

pub use local_whisper::LocalWhisper;

use crate::error::Result;
use crate::model::{AppContext, AudioChunk, Transcript};

pub trait Transcriber: Send + Sync {
    fn transcribe(&self, audio: &AudioChunk, ctx: &AppContext) -> Result<Transcript>;
    fn name(&self) -> &'static str;
}

pub struct FakeTranscriber {
    pub text: String,
}

impl FakeTranscriber {
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}

impl Transcriber for FakeTranscriber {
    fn transcribe(&self, _audio: &AudioChunk, _ctx: &AppContext) -> Result<Transcript> {
        Ok(Transcript::new(self.text.clone()))
    }

    fn name(&self) -> &'static str {
        "fake"
    }
}

/// Empty, or fully wrapped in `[...]` / `(...)` - whisper non-speech tokens.
pub fn is_non_speech(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() {
        return true;
    }
    let bytes = t.as_bytes();
    (bytes.first() == Some(&b'[') && bytes.last() == Some(&b']'))
        || (bytes.first() == Some(&b'(') && bytes.last() == Some(&b')'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_speech_detection() {
        assert!(is_non_speech(""));
        assert!(is_non_speech("  "));
        assert!(is_non_speech("[BLANK_AUDIO]"));
        assert!(is_non_speech("(upbeat music)"));
        assert!(!is_non_speech("hello"));
        assert!(!is_non_speech("[partial"));
    }
}
