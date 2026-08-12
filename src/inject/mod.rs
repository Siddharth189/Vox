mod autopaste;
mod clipboard;

pub use autopaste::AutoPasteInjector;
pub use clipboard::ClipboardInjector;

use crate::error::Result;
use crate::model::{AppContext, InjectionResult};

pub trait TextInjector: Send + Sync {
    fn inject(&self, text: &str, ctx: &AppContext) -> Result<InjectionResult>;
    fn name(&self) -> &'static str;
}

pub struct FakeTextInjector {
    pub last: std::sync::Mutex<Option<String>>,
}

impl FakeTextInjector {
    pub fn new() -> Self {
        Self {
            last: std::sync::Mutex::new(None),
        }
    }
}

impl Default for FakeTextInjector {
    fn default() -> Self {
        Self::new()
    }
}

impl TextInjector for FakeTextInjector {
    fn inject(&self, text: &str, _ctx: &AppContext) -> Result<InjectionResult> {
        *self.last.lock().unwrap() = Some(text.to_string());
        Ok(InjectionResult {
            injected_chars: text.chars().count(),
            strategy: "fake".into(),
        })
    }

    fn name(&self) -> &'static str {
        "fake"
    }
}
