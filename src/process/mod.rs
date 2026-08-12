mod dictionary;
mod ollama;
mod prompt;

pub use dictionary::normalize_processed_text;
pub use ollama::OllamaProcessor;
pub use prompt::{
    code_switch_fewshot, effective_system_prompt, fewshot, rewrite_only_user_message,
};

use crate::error::Result;
use crate::model::{EnrichedRequest, ProcessedText};

pub trait TextProcessor: Send + Sync {
    fn process(&self, req: &EnrichedRequest) -> Result<ProcessedText>;
    fn name(&self) -> &'static str;
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
