mod cleanup;
mod commands;
mod dictionary;
mod local;
mod ollama;
mod prompt;
mod repeats;

pub use cleanup::{Availability, CleanupProcessor};
pub use commands::{join_segments, Segment, VoiceCommands};
pub use dictionary::normalize_processed_text;
pub use local::{clean_text, LocalCleanup};
pub use ollama::{
    is_loopback_url, list_models, llm_url_permitted, probe as probe_llm, OllamaProcessor,
};
pub use prompt::{
    code_switch_fewshot, effective_system_prompt, fewshot, rewrite_only_user_message,
};
pub use repeats::collapse_repeats;

use crate::error::Result;
use crate::model::{EnrichedRequest, ProcessedText};

pub trait TextProcessor: Send + Sync {
    fn process(&self, req: &EnrichedRequest) -> Result<ProcessedText>;
    fn name(&self) -> &'static str;

    /// For processors that can use an LLM: whether it's reachable right
    /// now. `None` = no LLM configured.
    fn llm_status(&self) -> Option<bool> {
        None
    }
}

pub struct FakeTextProcessor {
    pub text: Option<String>,
    pub fail: bool,
}

impl FakeTextProcessor {
    pub fn passthrough() -> Self {
        Self {
            text: None,
            fail: false,
        }
    }

    pub fn returning(text: impl Into<String>) -> Self {
        Self {
            text: Some(text.into()),
            fail: false,
        }
    }

    pub fn failing() -> Self {
        Self {
            text: None,
            fail: true,
        }
    }
}

impl TextProcessor for FakeTextProcessor {
    fn process(&self, req: &EnrichedRequest) -> Result<ProcessedText> {
        if self.fail {
            return Err(crate::error::VoxError::Processing("fake failure".into()));
        }
        match &self.text {
            Some(t) => Ok(ProcessedText::new(t.clone())),
            None => Ok(ProcessedText::new(req.transcript.text.clone())),
        }
    }

    fn name(&self) -> &'static str {
        "fake"
    }
}
