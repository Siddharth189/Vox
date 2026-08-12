use std::process::Command;
use std::thread;
use std::time::Duration;

use arboard::Clipboard;

use crate::error::{Result, VoxError};
use crate::inject::TextInjector;
use crate::model::{AppContext, InjectionResult};
use crate::permissions;

#[cfg(target_os = "macos")]
const ACTIVATION_DELAY: Duration = Duration::from_millis(120);
const RESTORE_DELAY: Duration = Duration::from_millis(700);

#[cfg(target_os = "macos")]
const SYNTHESIZED_KEPT_STRATEGY: &str = "auto-paste sent (cg-event, clipboard kept)";
#[cfg(not(target_os = "macos"))]
const SYNTHESIZED_KEPT_STRATEGY: &str = "auto-paste sent (synthesized keystroke, clipboard kept)";

#[cfg(target_os = "macos")]
const UNAVAILABLE_STRATEGY: &str = "clipboard (grant Accessibility for auto-paste)";
#[cfg(not(target_os = "macos"))]
const UNAVAILABLE_STRATEGY: &str = "clipboard (auto-paste unavailable on this session)";

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

        // TCC can lie for locally-built/ad-hoc apps on macOS, and there is no
        // equivalent trust check on Linux at all - still try synthesize.
        if synthesize_paste().is_ok() {
            thread::sleep(RESTORE_DELAY);
            // Do not restore clipboard - keep dictated text as safety net.
            return Ok(InjectionResult {
                injected_chars: chars,
                strategy: SYNTHESIZED_KEPT_STRATEGY.into(),
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
            strategy: UNAVAILABLE_STRATEGY.into(),
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
    if try_enigo_paste_bounded().is_ok() {
        return Ok(());
    }
    cg_event_paste()
}

// enigo's Linux backend tries a portal-authorized libei session before
// falling back to plain X11/XTest. That portal negotiation is a blocking
// D-Bus round trip enigo makes no promise about the duration of: on a
// working setup it's near-instant once authorized, but on a compositor
// with no working RemoteDesktop portal backend (confirmed in testing: a
// broken permission-store lookup that never resolves, and this isn't
// KDE-specific - most non-GNOME/KDE Wayland compositors don't implement
// this portal interface for input at all) it can hang far longer than any
// single dictation should ever wait. Bound it so a broken or absent portal
// always falls through to the next strategy quickly instead of stalling
// the whole pipeline.
#[cfg(target_os = "macos")]
fn try_enigo_paste_bounded() -> Result<()> {
    try_enigo_paste()
}

#[cfg(not(target_os = "macos"))]
fn try_enigo_paste_bounded() -> Result<()> {
    const SYNTHESIS_TIMEOUT: Duration = Duration::from_secs(3);
    let (tx, rx) = std::sync::mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(try_enigo_paste());
    });
    match rx.recv_timeout(SYNTHESIS_TIMEOUT) {
        Ok(result) => result,
        Err(_) => Err(VoxError::Injection(
            "keystroke synthesis timed out (no working input portal for this session)".into(),
        )),
    }
}

fn try_enigo_paste() -> Result<()> {
    use enigo::{Direction, Enigo, Key, Keyboard, Settings};

    let mut enigo =
        Enigo::new(&Settings::default()).map_err(|e| VoxError::Injection(e.to_string()))?;

    // macOS pastes with Cmd+V; everywhere else it's Ctrl+V.
    #[cfg(target_os = "macos")]
    let modifier = Key::Meta;
    #[cfg(not(target_os = "macos"))]
    let modifier = Key::Control;

    // kVK_ANSI_V (9) is a macOS-only raw keycode. Elsewhere, enigo's portable
    // Unicode('v') maps to the right physical key for the active layout.
    #[cfg(target_os = "macos")]
    let paste_key = Key::Other(9);
    #[cfg(not(target_os = "macos"))]
    let paste_key = Key::Unicode('v');

    enigo
        .key(modifier, Direction::Press)
        .map_err(|e| VoxError::Injection(e.to_string()))?;
    enigo
        .key(paste_key, Direction::Click)
        .map_err(|e| VoxError::Injection(e.to_string()))?;
    thread::sleep(Duration::from_millis(100));
    enigo
        .key(modifier, Direction::Release)
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
