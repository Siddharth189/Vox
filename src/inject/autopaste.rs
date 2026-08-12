use std::process::Command;
use std::thread;
use std::time::Duration;

use arboard::Clipboard;

use crate::error::{Result, VoxError};
use crate::inject::TextInjector;
use crate::model::{AppContext, InjectionResult};
use crate::permissions;

const ACTIVATION_DELAY: Duration = Duration::from_millis(120);
const RESTORE_DELAY: Duration = Duration::from_millis(700);

pub struct AutoPasteInjector;

impl TextInjector for AutoPasteInjector {
    fn inject(&self, text: &str, ctx: &AppContext) -> Result<InjectionResult> {
        let mut clipboard =
            Clipboard::new().map_err(|e| VoxError::Injection(e.to_string()))?;
        let previous = clipboard.get_text().ok();

        clipboard
            .set_text(text.to_string())
            .map_err(|e| VoxError::Injection(e.to_string()))?;

        activate_target_app(ctx);

        let chars = text.chars().count();

        if permissions::accessibility_trusted() && synthesize_paste().is_ok() {
            thread::sleep(RESTORE_DELAY);
            if let Some(prev) = previous {
                let _ = clipboard.set_text(prev);
            }
            return Ok(InjectionResult {
                injected_chars: chars,
                strategy: "auto-paste (trusted cg-event)".into(),
            });
        }

        // TCC can lie for locally-built/ad-hoc apps — still try synthesize.
        if synthesize_paste().is_ok() {
            thread::sleep(RESTORE_DELAY);
            // Do not restore clipboard — keep dictated text as safety net.
            return Ok(InjectionResult {
                injected_chars: chars,
                strategy: "auto-paste sent (cg-event, clipboard kept)".into(),
            });
        }

        if osascript_paste().is_ok() {
            thread::sleep(RESTORE_DELAY);
            if let Some(prev) = previous {
                let _ = clipboard.set_text(prev);
            }
            return Ok(InjectionResult {
                injected_chars: chars,
                strategy: "auto-paste (osascript)".into(),
            });
        }

        Ok(InjectionResult {
            injected_chars: chars,
            strategy: "clipboard (grant Accessibility for auto-paste)".into(),
        })
    }

    fn name(&self) -> &'static str {
        "auto-paste"
    }
}

fn activate_target_app(ctx: &AppContext) {
    #[cfg(target_os = "macos")]
    {
        if ctx.bundle_id.is_empty() || ctx.bundle_id == "unknown" {
            return;
        }
        let script = format!(
            "tell application id \"{}\" to activate",
            ctx.bundle_id.replace('"', "")
        );
        let _ = Command::new("osascript").arg("-e").arg(&script).status();
        thread::sleep(ACTIVATION_DELAY);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = ctx;
    }
}

fn synthesize_paste() -> Result<()> {
    if try_enigo_paste().is_ok() {
        return Ok(());
    }
    cg_event_paste()
}

fn try_enigo_paste() -> Result<()> {
    use enigo::{Direction, Enigo, Key, Keyboard, Settings};

    let mut enigo =
        Enigo::new(&Settings::default()).map_err(|e| VoxError::Injection(e.to_string()))?;
    enigo
        .key(Key::Meta, Direction::Press)
        .map_err(|e| VoxError::Injection(e.to_string()))?;
    // kVK_ANSI_V = 9
    enigo
        .key(Key::Other(9), Direction::Click)
        .map_err(|e| VoxError::Injection(e.to_string()))?;
    thread::sleep(Duration::from_millis(100));
    enigo
        .key(Key::Meta, Direction::Release)
        .map_err(|e| VoxError::Injection(e.to_string()))?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn cg_event_paste() -> Result<()> {
    use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation, CGKeyCode};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| VoxError::Injection("CGEventSource failed".into()))?;
    let key_v: CGKeyCode = 9; // kVK_ANSI_V

    let down = CGEvent::new_keyboard_event(source.clone(), key_v, true)
        .map_err(|_| VoxError::Injection("CGEvent key-down failed".into()))?;
    down.set_flags(CGEventFlags::CGEventFlagCommand);
    down.post(CGEventTapLocation::HID);

    let up = CGEvent::new_keyboard_event(source, key_v, false)
        .map_err(|_| VoxError::Injection("CGEvent key-up failed".into()))?;
    up.set_flags(CGEventFlags::CGEventFlagCommand);
    up.post(CGEventTapLocation::HID);

    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn cg_event_paste() -> Result<()> {
    Err(VoxError::Injection("cg-event paste requires macOS".into()))
}

fn osascript_paste() -> Result<()> {
    let status = Command::new("osascript")
        .arg("-e")
        .arg("tell application \"System Events\" to keystroke \"v\" using command down")
        .status()
        .map_err(|e| VoxError::Injection(e.to_string()))?;
    if status.success() {
        Ok(())
    } else {
        Err(VoxError::Injection("osascript paste failed".into()))
    }
}
