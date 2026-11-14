#[cfg(target_os = "macos")]
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use arboard::Clipboard;

use crate::error::{Result, VoxError};
use crate::inject::{EditTarget, TextInjector};
use crate::model::{AppContext, InjectionResult};
use crate::permissions;

#[cfg(target_os = "macos")]
const ACTIVATION_DELAY: Duration = Duration::from_millis(120);
/// How long the target app gets to read the clipboard after Ctrl/Cmd+V
/// before the user's previous clipboard contents are put back. Electron apps
/// (Slack, VS Code, Discord) read it asynchronously, so this can't be tiny.
pub const RESTORE_DELAY: Duration = Duration::from_millis(700);
/// The dictation hotkey's modifiers may still be physically held when the
/// text is ready (e.g. the user stops a hands-free dictation with Ctrl+.
/// and keeps Ctrl down a moment). Pasting then sends Ctrl+Alt+V or
/// Ctrl+Shift+V instead of Ctrl+V, which is a different command (or AltGr+V,
/// a character) in many apps, so wait for them to come up first.
const MODIFIER_RELEASE_TIMEOUT: Duration = Duration::from_millis(1500);
/// How long the app gets to put its selection on the clipboard after
/// Ctrl/Cmd+C. Most do it within a few ms; Electron apps take longer.
const COPY_TIMEOUT: Duration = Duration::from_millis(600);

#[cfg(target_os = "macos")]
const TRUSTED_STRATEGY: &str = "auto-paste (trusted cg-event)";
#[cfg(windows)]
const TRUSTED_STRATEGY: &str = "auto-paste (SendInput)";
#[cfg(not(any(target_os = "macos", windows)))]
const TRUSTED_STRATEGY: &str = "auto-paste (synthesized keystroke)";

#[cfg(target_os = "macos")]
const SYNTHESIZED_KEPT_STRATEGY: &str = "auto-paste sent (cg-event, clipboard kept)";
#[cfg(not(target_os = "macos"))]
const SYNTHESIZED_KEPT_STRATEGY: &str = "auto-paste sent (synthesized keystroke, clipboard kept)";

#[cfg(target_os = "macos")]
const UNAVAILABLE_STRATEGY: &str = "clipboard (grant Accessibility for auto-paste)";
#[cfg(not(target_os = "macos"))]
const UNAVAILABLE_STRATEGY: &str = "clipboard (auto-paste unavailable on this session)";

pub struct AutoPasteInjector;

/// What was on the clipboard before Vox borrowed it.
enum SavedClipboard {
    Text(String),
    Image(arboard::ImageData<'static>),
    Nothing,
}

impl SavedClipboard {
    fn capture(clipboard: &mut Clipboard) -> Self {
        if let Ok(text) = clipboard.get_text() {
            return Self::Text(text);
        }
        if let Ok(image) = clipboard.get_image() {
            return Self::Image(image);
        }
        Self::Nothing
    }

    /// Puts the previous contents back, unless something else (the user, or
    /// another app) replaced our dictated text in the meantime.
    fn restore(self, clipboard: &mut Clipboard, dictated: &str) {
        let still_ours = clipboard.get_text().ok().as_deref() == Some(dictated);
        if still_ours {
            self.put_back(clipboard);
        }
    }

    fn put_back(self, clipboard: &mut Clipboard) {
        match self {
            Self::Text(prev) => {
                let _ = set_clipboard_text(clipboard, &prev);
            }
            Self::Image(img) => {
                let _ = clipboard.set_image(img);
            }
            Self::Nothing => {
                let _ = clipboard.clear();
            }
        }
    }
}

/// The user's clipboard, borrowed by edit by voice while it copies the
/// selection and pastes the result. Given back when released or dropped,
/// so a failed edit never leaves the clipboard holding Vox's marker.
pub struct ClipboardLease {
    saved: Option<SavedClipboard>,
    /// What Vox itself put on the clipboard: if it's still there, nobody
    /// else has used the clipboard since, and the original can go back.
    ours: Vec<String>,
}

impl ClipboardLease {
    /// Puts the user's clipboard back now, unless something else replaced it.
    pub fn release(mut self) {
        self.put_back_if_unchanged();
    }

    fn put_back_if_unchanged(&mut self) {
        let Some(saved) = self.saved.take() else {
            return;
        };
        if let Ok(mut clipboard) = Clipboard::new() {
            let current = clipboard.get_text().ok();
            if current.is_none_or(|c| self.ours.contains(&c)) {
                saved.put_back(&mut clipboard);
            }
        }
    }

    /// Hands the saved clipboard to the paste, which restores it later.
    fn take_saved(mut self) -> SavedClipboard {
        self.saved.take().unwrap_or(SavedClipboard::Nothing)
    }
}

impl Drop for ClipboardLease {
    fn drop(&mut self) {
        self.put_back_if_unchanged();
    }
}

impl TextInjector for AutoPasteInjector {
    /// Puts a unique marker on the clipboard and presses Ctrl/Cmd+C: if the
    /// marker is replaced, that's the selection; if it's still there,
    /// nothing was selected.
    fn begin_edit(&self, ctx: &AppContext) -> Result<EditTarget> {
        let mut clipboard = Clipboard::new().map_err(|e| VoxError::Injection(e.to_string()))?;
        let saved = SavedClipboard::capture(&mut clipboard);
        let marker = format!(
            "vox-edit-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        );
        set_clipboard_text(&mut clipboard, &marker)?;
        let mut lease = ClipboardLease {
            saved: Some(saved),
            ours: vec![marker.clone()],
        };

        activate_target_app(ctx);
        wait_for_modifiers_released(MODIFIER_RELEASE_TIMEOUT);
        // On failure the lease is dropped, which restores the clipboard.
        synthesize_copy()?;

        let start = Instant::now();
        let selection = loop {
            match clipboard.get_text() {
                Ok(text) if text != marker => break text,
                _ if start.elapsed() >= COPY_TIMEOUT => break String::new(),
                _ => thread::sleep(Duration::from_millis(15)),
            }
        };
        if !selection.is_empty() {
            lease.ours.push(selection.clone());
        }
        Ok(EditTarget {
            selection,
            lease: Some(lease),
        })
    }

    fn finish_edit(
        &self,
        target: EditTarget,
        text: &str,
        ctx: &AppContext,
    ) -> Result<InjectionResult> {
        match target.lease {
            // The clipboard held the marker or the copied selection, not
            // the user's; restore what was there before the edit started.
            Some(lease) => paste(text, ctx, lease.take_saved()),
            None => self.inject(text, ctx),
        }
    }

    fn inject(&self, text: &str, ctx: &AppContext) -> Result<InjectionResult> {
        let mut clipboard = Clipboard::new().map_err(|e| VoxError::Injection(e.to_string()))?;
        let previous = SavedClipboard::capture(&mut clipboard);
        drop(clipboard);
        paste(text, ctx, previous)
    }

    fn name(&self) -> &'static str {
        "auto-paste"
    }
}

/// Puts `text` on the clipboard, pastes it into the focused app, and gives
/// `previous` back once the app has read it.
fn paste(text: &str, ctx: &AppContext, previous: SavedClipboard) -> Result<InjectionResult> {
    {
        let mut clipboard = Clipboard::new().map_err(|e| VoxError::Injection(e.to_string()))?;
        set_clipboard_text(&mut clipboard, text)?;
    }

    activate_target_app(ctx);
    wait_for_modifiers_released(MODIFIER_RELEASE_TIMEOUT);

    let chars = text.chars().count();

    if permissions::accessibility_trusted() && synthesize_paste().is_ok() {
        restore_later(previous, text);
        return Ok(InjectionResult {
            injected_chars: chars,
            strategy: TRUSTED_STRATEGY.into(),
        });
    }

    // TCC can lie for locally-built/ad-hoc apps on macOS, and there is no
    // equivalent trust check on Linux at all - still try synthesize.
    if synthesize_paste().is_ok() {
        // Do not restore clipboard - keep dictated text as safety net.
        return Ok(InjectionResult {
            injected_chars: chars,
            strategy: SYNTHESIZED_KEPT_STRATEGY.into(),
        });
    }

    if osascript_paste().is_ok() {
        restore_later(previous, text);
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

/// Puts the previous clipboard back once the target app has had time to read
/// the paste, off the dictation's critical path (the text is already in the
/// app; nobody should wait for this).
fn restore_later(previous: SavedClipboard, dictated: &str) {
    let dictated = dictated.to_string();
    thread::spawn(move || {
        thread::sleep(RESTORE_DELAY);
        if let Ok(mut clipboard) = Clipboard::new() {
            previous.restore(&mut clipboard, &dictated);
        }
    });
}

/// Windows: keep dictations (and the restored previous contents) out of
/// Win+V clipboard history and cloud clipboard sync, the way password
/// managers do - otherwise every dictation piles up there.
#[cfg(windows)]
pub(crate) fn set_clipboard_text(clipboard: &mut Clipboard, text: &str) -> Result<()> {
    use arboard::SetExtWindows;
    clipboard
        .set()
        .exclude_from_history()
        .exclude_from_cloud()
        .text(text.to_string())
        .map_err(|e| VoxError::Injection(e.to_string()))
}

#[cfg(not(windows))]
pub(crate) fn set_clipboard_text(clipboard: &mut Clipboard, text: &str) -> Result<()> {
    clipboard
        .set_text(text.to_string())
        .map_err(|e| VoxError::Injection(e.to_string()))
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

/// Blocks until no modifier key is physically held, or `timeout` passes.
fn wait_for_modifiers_released(timeout: Duration) {
    let start = Instant::now();
    while modifiers_held() && start.elapsed() < timeout {
        thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(windows)]
fn modifiers_held() -> bool {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
    };
    [VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN]
        .iter()
        .any(|&vk| unsafe { GetAsyncKeyState(vk as i32) } as u16 & 0x8000 != 0)
}

#[cfg(target_os = "macos")]
fn modifiers_held() -> bool {
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn CGEventSourceFlagsState(state_id: i32) -> u64;
    }
    const HID_SYSTEM_STATE: i32 = 1;
    // Shift | Control | Option | Command
    const MODIFIER_MASK: u64 = 0x0002_0000 | 0x0004_0000 | 0x0008_0000 | 0x0010_0000;
    unsafe { CGEventSourceFlagsState(HID_SYSTEM_STATE) & MODIFIER_MASK != 0 }
}

/// No portable way to read physical key state on X11/Wayland without extra
/// deps; transcription itself takes long enough that keys are normally up.
#[cfg(not(any(target_os = "macos", windows)))]
fn modifiers_held() -> bool {
    false
}

fn synthesize_paste() -> Result<()> {
    if try_enigo_paste_bounded().is_ok() {
        return Ok(());
    }
    cg_event_paste()
}

/// Ctrl+C (Cmd+C on macOS) to the focused app, the same way as the paste.
fn synthesize_copy() -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        if try_enigo_shortcut(SHORTCUT_COPY).is_ok() {
            return Ok(());
        }
        let status = Command::new("osascript")
            .arg("-e")
            .arg("tell application \"System Events\" to keystroke \"c\" using command down")
            .status()
            .map_err(|e| VoxError::Injection(e.to_string()))?;
        if status.success() {
            Ok(())
        } else {
            Err(VoxError::Injection("couldn't send Cmd+C".into()))
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        try_enigo_shortcut_bounded(SHORTCUT_COPY)
    }
}

// enigo's Linux backend tries a portal-authorized libei session before
// falling back to plain X11/XTest. That portal negotiation is a blocking
// D-Bus round trip enigo makes no promise about the duration of: once the
// vendored restore_token patch (vendor/enigo) has a saved token, the
// compositor silently reuses prior consent and this resolves in well under
// a second. The first-ever call (or after the token is revoked) is
// different: the compositor pops up a real permission dialog and a human
// has to notice and click it, which routinely takes several seconds - and
// on a compositor with no working RemoteDesktop portal backend at all this
// can hang far longer than any single dictation should ever wait. Give the
// one-time consent flow a real chance to complete without stalling the
// common (already-authorized) case, which returns almost immediately
// either way.
fn try_enigo_paste_bounded() -> Result<()> {
    try_enigo_shortcut_bounded(SHORTCUT_PASTE)
}

#[cfg(any(target_os = "macos", windows))]
fn try_enigo_shortcut_bounded(shortcut: Shortcut) -> Result<()> {
    try_enigo_shortcut(shortcut)
}

#[cfg(not(any(target_os = "macos", windows)))]
fn try_enigo_shortcut_bounded(shortcut: Shortcut) -> Result<()> {
    const SYNTHESIS_TIMEOUT: Duration = Duration::from_secs(8);
    let (tx, rx) = std::sync::mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(try_enigo_shortcut(shortcut));
    });
    match rx.recv_timeout(SYNTHESIS_TIMEOUT) {
        Ok(result) => result,
        Err(_) => Err(VoxError::Injection(
            "keystroke synthesis timed out (no working input portal for this session, or consent dialog not yet approved)".into(),
        )),
    }
}

/// A Ctrl/Cmd shortcut: the letter, its macOS virtual key (kVK_ANSI_*),
/// and its Windows virtual key (VK_*).
#[derive(Clone, Copy)]
struct Shortcut {
    #[cfg_attr(any(target_os = "macos", windows), allow(dead_code))]
    letter: char,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    mac_key: u32,
    #[cfg_attr(not(windows), allow(dead_code))]
    windows_key: u32,
}

const SHORTCUT_PASTE: Shortcut = Shortcut {
    letter: 'v',
    mac_key: 9,
    windows_key: 0x56,
};
const SHORTCUT_COPY: Shortcut = Shortcut {
    letter: 'c',
    mac_key: 8,
    windows_key: 0x43,
};

fn try_enigo_shortcut(shortcut: Shortcut) -> Result<()> {
    use enigo::{Direction, Enigo, Key, Keyboard, Settings};

    let mut enigo =
        Enigo::new(&Settings::default()).map_err(|e| VoxError::Injection(e.to_string()))?;

    // macOS uses Cmd; everywhere else it's Ctrl.
    #[cfg(target_os = "macos")]
    let modifier = Key::Meta;
    #[cfg(not(target_os = "macos"))]
    let modifier = Key::Control;

    // Raw virtual keys on macOS and Windows: Ctrl+V is bound to the V
    // virtual key in every layout, while Unicode('v') has no key at all on
    // e.g. a Russian or Greek layout and enigo would fall back to typing a
    // literal "v". On Linux, enigo maps the letter through the active XKB
    // keymap, which is what XTest/libei expect.
    #[cfg(target_os = "macos")]
    let key = Key::Other(shortcut.mac_key);
    #[cfg(windows)]
    let key = Key::Other(shortcut.windows_key);
    #[cfg(not(any(target_os = "macos", windows)))]
    let key = Key::Unicode(shortcut.letter);

    enigo
        .key(modifier, Direction::Press)
        .map_err(|e| VoxError::Injection(e.to_string()))?;
    let clicked = enigo.key(key, Direction::Click);
    thread::sleep(Duration::from_millis(30));
    // Always release the modifier, even if the V click failed, so Ctrl is
    // never left logically stuck down.
    let released = enigo.key(modifier, Direction::Release);
    clicked.map_err(|e| VoxError::Injection(e.to_string()))?;
    released.map_err(|e| VoxError::Injection(e.to_string()))?;
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

#[cfg(target_os = "macos")]
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

#[cfg(not(target_os = "macos"))]
fn osascript_paste() -> Result<()> {
    Err(VoxError::Injection("osascript paste requires macOS".into()))
}
