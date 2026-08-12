//! Linux/dev stub: real Whisper (Metal) lives in `local_whisper.rs` on macOS.
use std::path::Path;

use crate::error::{Result, VoxError};
use crate::model::{AppContext, AudioChunk, Transcript};
use crate::stt::Transcriber;

pub struct LocalWhisper {
    input_language: Option<String>,
    initial_prompt: Option<String>,
}

impl LocalWhisper {
    pub fn new(model_path: impl AsRef<Path>) -> Result<Self> {
        let path = model_path.as_ref();
        if !path.exists() {
            return Err(VoxError::Transcription(format!(
                "whisper model not found at {}; run scripts/download_model.sh",
                path.display()
            )));
        }
        Ok(Self {
            input_language: None,
            initial_prompt: None,
        })
    }

    pub fn with_input_language(mut self, lang: impl Into<String>) -> Self {
        self.input_language = Some(lang.into());
        self
    }

    pub fn with_initial_prompt(mut self, dictionary: &[String]) -> Self {
        let joined: String = dictionary
            .iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(", ");
        self.initial_prompt = if joined.is_empty() {
            None
        } else {
            Some(joined.chars().take(200).collect())
        };
        self
    }
}

impl Transcriber for LocalWhisper {
    fn transcribe(&self, audio: &AudioChunk, _ctx: &AppContext) -> Result<Transcript> {
        let _ = (&self.input_language, &self.initial_prompt);
        if audio.samples.is_empty() {
            return Ok(Transcript::new(""));
        }
        Err(VoxError::Transcription(
            "LocalWhisper requires macOS (whisper-rs Metal); use `vox demo` / FakeTranscriber on this host"
                .into(),
        ))
    }

    fn name(&self) -> &'static str {
        "local-whisper-stub"
    }
}
