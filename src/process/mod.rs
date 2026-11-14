mod cleanup;
mod commands;
mod dictionary;
pub mod edit;
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

use crate::error::{Result, VoxError};
use crate::model::{AppContext, EnrichedRequest, ProcessedText};

pub trait TextProcessor: Send + Sync {
    fn process(&self, req: &EnrichedRequest) -> Result<ProcessedText>;
    fn name(&self) -> &'static str;

    /// For processors that can use an LLM: whether it's reachable right
    /// now. `None` = no LLM configured.
    fn llm_status(&self) -> Option<bool> {
        None
    }

    /// Like `llm_status`, for edit by voice, which uses the LLM whether or
    /// not dictations do. `None` = no LLM server set up.
    fn edit_status(&self) -> Option<bool> {
        None
    }

    /// Edit by voice: `selection` rewritten as the spoken `instruction`
    /// says (or, with nothing selected, the text it asks for). Only an LLM
    /// can do this; there is no rule-based fallback.
    fn edit(&self, instruction: &str, selection: &str, ctx: &AppContext) -> Result<String> {
        let _ = (instruction, selection, ctx);
        Err(VoxError::Processing(
            "edit by voice needs an LLM server set up in settings".into(),
        ))
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
            return Err(VoxError::Processing("fake failure".into()));
        }
        match &self.text {
            Some(t) => Ok(ProcessedText::new(t.clone())),
            None => Ok(ProcessedText::new(req.transcript.text.clone())),
        }
    }

    /// `text` when set, otherwise the instruction and selection joined, so
    /// tests can see what reached the processor.
    fn edit(&self, instruction: &str, selection: &str, _ctx: &AppContext) -> Result<String> {
        if self.fail {
            return Err(VoxError::Processing("fake failure".into()));
        }
        Ok(match &self.text {
            Some(t) => t.clone(),
            None => format!("{instruction}: {selection}"),
        })
    }

    fn name(&self) -> &'static str {
        "fake"
    }
}
