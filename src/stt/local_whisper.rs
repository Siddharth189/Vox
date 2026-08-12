use std::path::Path;

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::audio::TARGET_RATE;
use crate::error::{Result, VoxError};
use crate::model::{AppContext, AudioChunk, Transcript};
use crate::stt::Transcriber;

const MAX_INITIAL_PROMPT_LEN: usize = 200;

pub struct LocalWhisper {
    ctx: WhisperContext,
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
        let ctx = WhisperContext::new_with_params(
            path.to_string_lossy().as_ref(),
            WhisperContextParameters::default(),
        )
        .map_err(|e| VoxError::Transcription(e.to_string()))?;
        Ok(Self {
            ctx,
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

        let mut state = self
            .ctx
            .create_state()
            .map_err(|e| VoxError::Transcription(e.to_string()))?;

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);

        if let Some(ref lang) = self.input_language {
            if let Some(code) = whisper_language_code(lang) {
                params.set_language(Some(code));
            }
        }

        if let Some(ref prompt) = self.initial_prompt {
            params.set_initial_prompt(prompt);
        }

        state
            .full(params, &audio.samples)
            .map_err(|e| VoxError::Transcription(e.to_string()))?;

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

        Ok(Transcript::new(text.trim()))
    }

    fn name(&self) -> &'static str {
        "local-whisper"
    }
}

pub fn whisper_language_code(name: &str) -> Option<&'static str> {
    match name.trim().to_ascii_lowercase().as_str() {
        "english" => Some("en"),
        "hindi" => Some("hi"),
        "tamil" => Some("ta"),
        "spanish" => Some("es"),
        "japanese" => Some("ja"),
        "french" => Some("fr"),
        "german" => Some("de"),
        "portuguese" => Some("pt"),
        "chinese" => Some("zh"),
        "arabic" => Some("ar"),
        "korean" => Some("ko"),
        "italian" => Some("it"),
        "russian" => Some("ru"),
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
        assert_eq!(whisper_language_code("Hinglish / mixed Hindi-English"), None);
    }
}
