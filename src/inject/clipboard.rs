use arboard::Clipboard;

use crate::error::{Result, VoxError};
use crate::inject::TextInjector;
use crate::model::{AppContext, InjectionResult};

pub struct ClipboardInjector;

impl TextInjector for ClipboardInjector {
    fn inject(&self, text: &str, _ctx: &AppContext) -> Result<InjectionResult> {
        let mut clipboard =
            Clipboard::new().map_err(|e| VoxError::Injection(e.to_string()))?;
        clipboard
            .set_text(text.to_string())
            .map_err(|e| VoxError::Injection(e.to_string()))?;
        Ok(InjectionResult {
            injected_chars: text.chars().count(),
            strategy: "clipboard".into(),
        })
    }

    fn name(&self) -> &'static str {
        "clipboard"
    }
}
