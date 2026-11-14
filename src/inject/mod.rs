mod autopaste;
mod clipboard;

pub use autopaste::{AutoPasteInjector, ClipboardLease, RESTORE_DELAY};

/// One-shot CLI commands exit right after pasting; give the background
/// clipboard restore time to run first.
pub fn wait_for_clipboard_restore(strategy: &str) {
    if strategy.starts_with("auto-paste") {
        std::thread::sleep(RESTORE_DELAY + std::time::Duration::from_millis(200));
    }
}
pub use clipboard::ClipboardInjector;

use crate::error::{Result, VoxError};
use crate::model::{AppContext, InjectionResult};

/// The selection copied for edit by voice, and the user's clipboard, which
/// stays borrowed until the edit is pasted (`finish_edit`) or dropped
/// (`abort_edit`).
pub struct EditTarget {
    /// The selected text; empty when nothing was selected.
    pub selection: String,
    lease: Option<ClipboardLease>,
}

impl EditTarget {
    /// A target with no clipboard borrowed (fakes, tests).
    pub fn detached(selection: impl Into<String>) -> Self {
        Self {
            selection: selection.into(),
            lease: None,
        }
    }
}

pub trait TextInjector: Send + Sync {
    fn inject(&self, text: &str, ctx: &AppContext) -> Result<InjectionResult>;
    fn name(&self) -> &'static str;

    /// Edit by voice: copies the selection in the focused app. Needs
    /// synthesized keystrokes, so the clipboard-only injector can't.
    fn begin_edit(&self, ctx: &AppContext) -> Result<EditTarget> {
        let _ = ctx;
        Err(VoxError::Injection(
            "edit by voice needs auto-paste turned on in settings".into(),
        ))
    }

    /// Pastes `text` over the selection `begin_edit` copied, then gives the
    /// user's clipboard back.
    fn finish_edit(
        &self,
        target: EditTarget,
        text: &str,
        ctx: &AppContext,
    ) -> Result<InjectionResult> {
        let _ = target;
        self.inject(text, ctx)
    }

    /// The edit isn't happening after all: gives the clipboard back.
    fn abort_edit(&self, target: EditTarget) {
        if let Some(lease) = target.lease {
            lease.release();
        }
    }
}

pub struct FakeTextInjector {
    pub last: std::sync::Mutex<Option<String>>,
    /// What `begin_edit` reports as selected.
    pub selection: String,
}

impl FakeTextInjector {
    pub fn new() -> Self {
        Self {
            last: std::sync::Mutex::new(None),
            selection: String::new(),
        }
    }

    pub fn with_selection(selection: impl Into<String>) -> Self {
        Self {
            selection: selection.into(),
            ..Self::new()
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

    fn begin_edit(&self, _ctx: &AppContext) -> Result<EditTarget> {
        Ok(EditTarget::detached(self.selection.clone()))
    }
}
