//! Hotkey parsing and the push-to-talk state machine.
//!
//! Kept free of any OS or event-loop types (beyond the `Code` enum) so the
//! timing rules that decide when a dictation starts and stops are unit-tested
//! instead of only being exercised by hand.

use std::str::FromStr;
use std::time::{Duration, Instant};

use global_hotkey::hotkey::{Code, HotKey, Modifiers};

use crate::config::{HotkeyConfig, HotkeyMode};
use crate::error::{Result, VoxError};

/// A press-and-release shorter than this is a "tap": in hybrid mode it
/// latches recording on (hands-free) until the hotkey is pressed again.
/// Anything held longer is push-to-talk and stops on release.
pub const TAP_THRESHOLD: Duration = Duration::from_millis(350);

/// Parses a key name from settings into a `Code`. Accepts the W3C
/// `KeyboardEvent.code` names the settings UI records (`Period`, `KeyA`,
/// `Digit1`, `F5`, ...) plus the obvious hand-typed forms (`.`, `period`,
/// `a`, `1`, `space`).
pub fn parse_key(name: &str) -> Option<Code> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return None;
    }
    let alias = match trimmed.to_ascii_lowercase().as_str() {
        "." | "period" | "dot" | "full stop" => Some(Code::Period),
        "," | "comma" => Some(Code::Comma),
        "/" | "slash" => Some(Code::Slash),
        "\\" | "backslash" => Some(Code::Backslash),
        ";" | "semicolon" => Some(Code::Semicolon),
        "'" | "quote" | "apostrophe" => Some(Code::Quote),
        "`" | "backquote" | "backtick" | "grave" => Some(Code::Backquote),
        "-" | "minus" | "dash" => Some(Code::Minus),
        "=" | "equal" | "equals" => Some(Code::Equal),
        "[" | "bracketleft" => Some(Code::BracketLeft),
        "]" | "bracketright" => Some(Code::BracketRight),
        " " | "space" | "spacebar" => Some(Code::Space),
        "enter" | "return" => Some(Code::Enter),
        "tab" => Some(Code::Tab),
        "esc" | "escape" => Some(Code::Escape),
        "backspace" => Some(Code::Backspace),
        "insert" | "ins" => Some(Code::Insert),
        "delete" | "del" => Some(Code::Delete),
        "home" => Some(Code::Home),
        "end" => Some(Code::End),
        "pageup" => Some(Code::PageUp),
        "pagedown" => Some(Code::PageDown),
        "up" | "arrowup" => Some(Code::ArrowUp),
        "down" | "arrowdown" => Some(Code::ArrowDown),
        "left" | "arrowleft" => Some(Code::ArrowLeft),
        "right" | "arrowright" => Some(Code::ArrowRight),
        _ => None,
    };
    if alias.is_some() {
        return alias;
    }

    let mut chars = trimmed.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        if c.is_ascii_alphabetic() {
            return Code::from_str(&format!("Key{}", c.to_ascii_uppercase())).ok();
        }
        if c.is_ascii_digit() {
            return Code::from_str(&format!("Digit{c}")).ok();
        }
    }

    if let Ok(code) = Code::from_str(trimmed) {
        return Some(code);
    }
    // "f5" -> "F5", "keya" -> "KeyA", "numpad1" -> "Numpad1"
    let mut title = String::with_capacity(trimmed.len());
    let mut upper_next = true;
    for c in trimmed.chars() {
        if upper_next {
            title.extend(c.to_uppercase());
            upper_next = false;
        } else {
            title.extend(c.to_lowercase());
        }
    }
    if let Some(rest) = title.strip_prefix("Key") {
        if rest.len() == 1 {
            return Code::from_str(&format!("Key{}", rest.to_ascii_uppercase())).ok();
        }
    }
    Code::from_str(&title).ok()
}

/// The canonical spelling written back to settings.yaml, e.g. "." -> "Period".
pub fn canonical_key_name(name: &str) -> Option<String> {
    parse_key(name).map(|c| c.to_string())
}

fn parse_modifiers(names: &[String]) -> Result<Modifiers> {
    let mut mods = Modifiers::empty();
    let mut has_primary = false;
    for m in names {
        match m.trim().to_ascii_lowercase().as_str() {
            "control" | "ctrl" => {
                mods |= Modifiers::CONTROL;
                has_primary = true;
            }
            "alt" | "option" => {
                mods |= Modifiers::ALT;
                has_primary = true;
            }
            "shift" => mods |= Modifiers::SHIFT,
            "super" | "cmd" | "command" | "win" | "windows" | "meta" => {
                mods |= Modifiers::SUPER;
                has_primary = true;
            }
            "" => {}
            other => {
                return Err(VoxError::Config(format!(
                    "unknown hotkey modifier `{other}`"
                )));
            }
        }
    }
    if !has_primary {
        return Err(VoxError::Config(
            "hotkey requires control, alt, or super (shift alone is rejected)".into(),
        ));
    }
    Ok(mods)
}

/// Builds the `HotKey` described by `cfg`, or a clear error if any part of it
/// is invalid (instead of silently binding something else).
pub fn hotkey_from_config(cfg: &HotkeyConfig) -> Result<HotKey> {
    let mods = parse_modifiers(&cfg.modifiers)?;
    let code = parse_key(&cfg.key)
        .ok_or_else(|| VoxError::Config(format!("unknown hotkey key `{}`", cfg.key)))?;
    Ok(HotKey::new(Some(mods), code))
}

/// Human-readable label, e.g. "Ctrl+." or "Ctrl+Alt+Space".
pub fn display_label(cfg: &HotkeyConfig) -> String {
    let mut parts: Vec<String> = Vec::new();
    for m in &cfg.modifiers {
        let label = match m.trim().to_ascii_lowercase().as_str() {
            "control" | "ctrl" => "Ctrl",
            "alt" | "option" => {
                if cfg!(target_os = "macos") {
                    "Option"
                } else {
                    "Alt"
                }
            }
            "shift" => "Shift",
            "super" | "cmd" | "command" | "win" | "windows" | "meta" => {
                if cfg!(target_os = "macos") {
                    "Cmd"
                } else if cfg!(windows) {
                    "Win"
                } else {
                    "Super"
                }
            }
            _ => continue,
        };
        parts.push(label.to_string());
    }
    let key = match parse_key(&cfg.key) {
        Some(Code::Period) => ".".to_string(),
        Some(Code::Comma) => ",".to_string(),
        Some(Code::Slash) => "/".to_string(),
        Some(Code::Semicolon) => ";".to_string(),
        Some(Code::Quote) => "'".to_string(),
        Some(Code::Backquote) => "`".to_string(),
        Some(Code::Minus) => "-".to_string(),
        Some(Code::Equal) => "=".to_string(),
        Some(code) => {
            let s = code.to_string();
            s.strip_prefix("Key")
                .or_else(|| s.strip_prefix("Digit"))
                .map(str::to_string)
                .unwrap_or(s)
        }
        None => cfg.key.clone(),
    };
    parts.push(key);
    parts.join("+")
}

/// What the tray should do in response to a hotkey edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PttAction {
    None,
    Start,
    Stop,
}

/// Push-to-talk state machine. `Hybrid` (the default) gives both behaviours
/// people expect from a dictation key without a setting: tap to toggle
/// hands-free listening, or hold to talk and release to finish.
#[derive(Debug)]
pub struct PushToTalk {
    mode: HotkeyMode,
    recording: bool,
    latched: bool,
    pressed_at: Option<Instant>,
    /// The press that stopped a latched recording also produces a release;
    /// swallow it so it can't be mistaken for anything else.
    swallow_release: bool,
}

impl PushToTalk {
    pub fn new(mode: HotkeyMode) -> Self {
        Self {
            mode,
            recording: false,
            latched: false,
            pressed_at: None,
            swallow_release: false,
        }
    }

    pub fn set_mode(&mut self, mode: HotkeyMode) {
        if self.mode != mode {
            *self = Self::new(mode);
        }
    }

    pub fn is_recording(&self) -> bool {
        self.recording
    }

    /// True while recording hands-free (after a tap, or in toggle mode).
    pub fn is_latched(&self) -> bool {
        self.recording && self.latched
    }

    pub fn on_press(&mut self, now: Instant) -> PttAction {
        if self.recording {
            // Key auto-repeat while held produces extra presses; only a
            // press after the key was released (latched) means "stop".
            if self.latched {
                self.reset();
                self.swallow_release = true;
                return PttAction::Stop;
            }
            return PttAction::None;
        }
        self.recording = true;
        self.latched = self.mode == HotkeyMode::Toggle;
        self.pressed_at = Some(now);
        self.swallow_release = false;
        PttAction::Start
    }

    pub fn on_release(&mut self, now: Instant) -> PttAction {
        if self.swallow_release {
            self.swallow_release = false;
            return PttAction::None;
        }
        if !self.recording || self.latched {
            return PttAction::None;
        }
        let held = self
            .pressed_at
            .map(|t| now.saturating_duration_since(t))
            .unwrap_or(Duration::ZERO);
        match self.mode {
            HotkeyMode::Hybrid if held < TAP_THRESHOLD => {
                self.latched = true;
                PttAction::None
            }
            HotkeyMode::Toggle => PttAction::None,
            _ => {
                self.reset();
                PttAction::Stop
            }
        }
    }

    /// Recording ended for some other reason (cancel, max duration, error).
    pub fn reset(&mut self) {
        self.recording = false;
        self.latched = false;
        self.pressed_at = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(mods: &[&str], key: &str) -> HotkeyConfig {
        HotkeyConfig {
            enabled: true,
            modifiers: mods.iter().map(|s| s.to_string()).collect(),
            key: key.into(),
            mode: HotkeyMode::Hybrid,
        }
    }

    #[test]
    fn parses_period_in_every_spelling() {
        for name in [".", "Period", "period", "PERIOD", "dot", " . "] {
            assert_eq!(parse_key(name), Some(Code::Period), "{name:?}");
        }
    }

    #[test]
    fn parses_letters_digits_and_function_keys() {
        assert_eq!(parse_key("a"), Some(Code::KeyA));
        assert_eq!(parse_key("KeyA"), Some(Code::KeyA));
        assert_eq!(parse_key("keya"), Some(Code::KeyA));
        assert_eq!(parse_key("7"), Some(Code::Digit7));
        assert_eq!(parse_key("Digit7"), Some(Code::Digit7));
        assert_eq!(parse_key("F5"), Some(Code::F5));
        assert_eq!(parse_key("f12"), Some(Code::F12));
        assert_eq!(parse_key("Space"), Some(Code::Space));
        assert_eq!(parse_key("space"), Some(Code::Space));
    }

    #[test]
    fn unknown_keys_are_errors_not_silent_space() {
        assert_eq!(parse_key("definitely-not-a-key"), None);
        assert_eq!(parse_key(""), None);
        assert!(hotkey_from_config(&cfg(&["control"], "nope")).is_err());
    }

    #[test]
    fn ctrl_period_builds_expected_hotkey() {
        let hk = hotkey_from_config(&cfg(&["control"], ".")).unwrap();
        assert_eq!(hk, HotKey::new(Some(Modifiers::CONTROL), Code::Period));
    }

    #[test]
    fn shift_only_and_unknown_modifiers_rejected() {
        assert!(hotkey_from_config(&cfg(&["shift"], "Period")).is_err());
        assert!(hotkey_from_config(&cfg(&["control", "hyper"], "Period")).is_err());
    }

    #[test]
    fn canonical_names() {
        assert_eq!(canonical_key_name(".").as_deref(), Some("Period"));
        assert_eq!(canonical_key_name("a").as_deref(), Some("KeyA"));
        assert_eq!(canonical_key_name("bogus"), None);
    }

    #[test]
    fn labels_are_readable() {
        assert_eq!(display_label(&cfg(&["control"], "Period")), "Ctrl+.");
        assert_eq!(
            display_label(&cfg(&["control", "shift"], "KeyD")),
            "Ctrl+Shift+D"
        );
    }

    #[test]
    fn hybrid_hold_stops_on_release() {
        let t0 = Instant::now();
        let mut ptt = PushToTalk::new(HotkeyMode::Hybrid);
        assert_eq!(ptt.on_press(t0), PttAction::Start);
        // auto-repeat while held is ignored
        assert_eq!(
            ptt.on_press(t0 + Duration::from_millis(500)),
            PttAction::None
        );
        assert_eq!(ptt.on_release(t0 + Duration::from_secs(2)), PttAction::Stop);
        assert!(!ptt.is_recording());
    }

    #[test]
    fn hybrid_tap_latches_until_next_press() {
        let t0 = Instant::now();
        let mut ptt = PushToTalk::new(HotkeyMode::Hybrid);
        assert_eq!(ptt.on_press(t0), PttAction::Start);
        assert_eq!(
            ptt.on_release(t0 + Duration::from_millis(120)),
            PttAction::None
        );
        assert!(ptt.is_latched());
        assert_eq!(ptt.on_press(t0 + Duration::from_secs(5)), PttAction::Stop);
        // the release of the stopping press must not start or stop anything
        assert_eq!(
            ptt.on_release(t0 + Duration::from_millis(5100)),
            PttAction::None
        );
        assert!(!ptt.is_recording());
        // and the next press starts a fresh dictation
        assert_eq!(ptt.on_press(t0 + Duration::from_secs(8)), PttAction::Start);
    }

    #[test]
    fn hold_mode_never_latches() {
        let t0 = Instant::now();
        let mut ptt = PushToTalk::new(HotkeyMode::Hold);
        assert_eq!(ptt.on_press(t0), PttAction::Start);
        assert_eq!(
            ptt.on_release(t0 + Duration::from_millis(50)),
            PttAction::Stop
        );
    }

    #[test]
    fn toggle_mode_ignores_release() {
        let t0 = Instant::now();
        let mut ptt = PushToTalk::new(HotkeyMode::Toggle);
        assert_eq!(ptt.on_press(t0), PttAction::Start);
        assert_eq!(ptt.on_release(t0 + Duration::from_secs(3)), PttAction::None);
        assert_eq!(ptt.on_press(t0 + Duration::from_secs(4)), PttAction::Stop);
        assert_eq!(
            ptt.on_release(t0 + Duration::from_millis(4100)),
            PttAction::None
        );
    }

    #[test]
    fn reset_clears_latched_recording() {
        let t0 = Instant::now();
        let mut ptt = PushToTalk::new(HotkeyMode::Toggle);
        ptt.on_press(t0);
        ptt.reset();
        assert!(!ptt.is_recording());
        assert_eq!(ptt.on_press(t0 + Duration::from_secs(1)), PttAction::Start);
    }
}
