use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Result, VoxError};
use crate::model::{AppProfile, Format, Privacy};
use crate::secure_fs;

/// Where settings, history, logs, and models live. `VOX_DATA_DIR` overrides
/// the platform default (portable installs, tests).
pub fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("VOX_DATA_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    platform_data_dir()
}

#[cfg(target_os = "macos")]
fn platform_data_dir() -> PathBuf {
    match std::env::var_os("HOME") {
        Some(home) => PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join("vox"),
        None => PathBuf::from("."),
    }
}

/// %LOCALAPPDATA%\Vox rather than the roaming %APPDATA%: models are hundreds
/// of MB and dictation history shouldn't sync to other machines.
#[cfg(windows)]
fn platform_data_dir() -> PathBuf {
    match std::env::var_os("LOCALAPPDATA") {
        Some(local) if !local.is_empty() => PathBuf::from(local).join("Vox"),
        _ => match std::env::var_os("USERPROFILE") {
            Some(home) => PathBuf::from(home).join("AppData").join("Local").join("Vox"),
            None => PathBuf::from("."),
        },
    }
}

/// XDG Base Directory Specification: $XDG_DATA_HOME, falling back to ~/.local/share.
#[cfg(not(any(target_os = "macos", windows)))]
fn platform_data_dir() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("vox");
        }
    }
    match std::env::var_os("HOME") {
        Some(home) => PathBuf::from(home).join(".local").join("share").join("vox"),
        None => PathBuf::from("."),
    }
}

pub fn model_dir() -> PathBuf {
    data_dir().join("models")
}

pub fn settings_path() -> PathBuf {
    data_dir().join("settings.yaml")
}

pub fn log_path() -> PathBuf {
    data_dir().join("tray.log")
}

#[cfg(windows)]
pub fn total_ram_bytes() -> Option<u64> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    if unsafe { GlobalMemoryStatusEx(&mut status) } != 0 {
        Some(status.ullTotalPhys)
    } else {
        None
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
pub fn total_ram_bytes() -> Option<u64> {
    let contents = std::fs::read_to_string("/proc/meminfo").ok()?;
    let line = contents.lines().find(|l| l.starts_with("MemTotal:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

/// Below this, Vox trims the LLM system prompt (see process/prompt.rs) since
/// prompt evaluation time scales with token count: measured on a real
/// 8GB/4-core machine, the full prompt (421 tokens including few-shot
/// examples) took 46.6s to evaluate alone with a 1B model, consuming 82% of
/// a 57s total request - independent of which small model is configured,
/// since any model of similar size has to process the same token count.
/// Same 10GB line as the LLM-model-size and Whisper-model-size heuristics
/// elsewhere, for one consistent "is this a constrained machine" answer.
#[cfg(not(target_os = "macos"))]
pub const LOW_RAM_THRESHOLD_BYTES: u64 = 10_000_000_000;

#[cfg(not(target_os = "macos"))]
pub fn low_resource_mode() -> bool {
    total_ram_bytes().is_some_and(|b| b < LOW_RAM_THRESHOLD_BYTES)
}

/// How the dictation hotkey behaves.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum HotkeyMode {
    /// Tap to start hands-free and tap again to finish, or hold to talk and
    /// release to finish.
    #[default]
    Hybrid,
    /// Strict push-to-talk: recording lasts exactly as long as the key is held.
    Hold,
    /// Every press toggles recording on/off.
    Toggle,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HotkeyConfig {
    pub enabled: bool,
    pub modifiers: Vec<String>,
    pub key: String,
    #[serde(default)]
    pub mode: HotkeyMode,
}

pub const DEFAULT_HOTKEY_MODIFIERS: &[&str] = &["control"];
pub const DEFAULT_HOTKEY_KEY: &str = "Period";

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            modifiers: DEFAULT_HOTKEY_MODIFIERS.iter().map(|m| m.to_string()).collect(),
            key: DEFAULT_HOTKEY_KEY.into(),
            mode: HotkeyMode::Hybrid,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProfileConfig {
    #[serde(default)]
    pub format: Format,
    #[serde(default)]
    pub privacy: Privacy,
}

impl From<&ProfileConfig> for AppProfile {
    fn from(p: &ProfileConfig) -> Self {
        AppProfile {
            format: p.format,
            privacy: p.privacy,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Settings {
    #[serde(default = "default_true")]
    pub auto_paste: bool,
    #[serde(default = "default_auto")]
    pub output_language: String,
    #[serde(default = "default_auto")]
    pub input_language: String,
    #[serde(default = "default_whisper_model")]
    pub whisper_model: String,
    #[serde(default = "default_llm_model")]
    pub llm_model: String,
    #[serde(default)]
    pub custom_dictionary: Vec<String>,
    #[serde(default)]
    pub custom_aliases: HashMap<String, Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_message_override: Option<String>,
    #[serde(default)]
    pub hotkey: HotkeyConfig,
    /// Polish text with the local Ollama model when it's running. When it
    /// isn't (or this is off), Vox uses its built-in rule-based cleanup, which
    /// adds no latency and needs no extra install.
    #[serde(default = "default_true")]
    pub llm_cleanup: bool,
    /// Short start/stop tones so you know when Vox is listening without
    /// looking at the tray.
    #[serde(default = "default_true")]
    pub sound_cues: bool,
    /// Microphone to record from (case-insensitive name match). None, or a
    /// name that isn't connected right now, means the system default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_device: Option<String>,
    pub profiles: HashMap<String, ProfileConfig>,
}

fn default_true() -> bool {
    true
}

fn default_auto() -> String {
    "auto".into()
}

/// Whisper model downloaded on first run. Quantized `base` (multilingual,
/// ~57 MB) transcribes a 5 s utterance in well under a second on a laptop
/// CPU; `vox download-model small-q5_1` trades ~3x the time for accuracy.
pub const DEFAULT_WHISPER_MODEL_NAME: &str = "base-q5_1";

pub fn whisper_model_path(name: &str) -> PathBuf {
    model_dir().join(format!("ggml-{name}.bin"))
}

fn default_whisper_model() -> String {
    whisper_model_path(DEFAULT_WHISPER_MODEL_NAME)
        .to_string_lossy()
        .into_owned()
}

fn default_llm_model() -> String {
    "llama3.2:latest".into()
}

pub fn is_blocked_llm_model(model: &str) -> bool {
    let needle = ["qw", "en"].concat();
    model.to_ascii_lowercase().contains(&needle)
}

impl Settings {
    pub fn with_defaults() -> Self {
        let mut profiles = HashMap::new();
        profiles.insert(
            "default".into(),
            ProfileConfig {
                format: Format::CleanProse,
                privacy: Privacy::LocalOnly,
            },
        );
        profiles.insert(
            "com.tinyspeck.slackmacgap".into(),
            ProfileConfig {
                format: Format::Casual,
                privacy: Privacy::LocalOnly,
            },
        );
        profiles.insert(
            "com.microsoft.VSCode".into(),
            ProfileConfig {
                format: Format::CodeContext,
                privacy: Privacy::LocalOnly,
            },
        );
        profiles.insert(
            "com.apple.Terminal".into(),
            ProfileConfig {
                format: Format::Shell,
                privacy: Privacy::LocalOnly,
            },
        );
        profiles.insert(
            "org.alacritty".into(),
            ProfileConfig {
                format: Format::Shell,
                privacy: Privacy::LocalOnly,
            },
        );
        profiles.insert(
            "com.1password.1password".into(),
            ProfileConfig {
                format: Format::CleanProse,
                privacy: Privacy::Disabled,
            },
        );

        // Linux has no bundle IDs; the app-detector keys profiles by WM_CLASS
        // instead, so these are additive, not a replacement for the macOS
        // entries above (harmless there too - the strings just never match).
        profiles.insert(
            "Slack".into(),
            ProfileConfig {
                format: Format::Casual,
                privacy: Privacy::LocalOnly,
            },
        );
        profiles.insert(
            "Code".into(),
            ProfileConfig {
                format: Format::CodeContext,
                privacy: Privacy::LocalOnly,
            },
        );
        profiles.insert(
            "Alacritty".into(),
            ProfileConfig {
                format: Format::Shell,
                privacy: Privacy::LocalOnly,
            },
        );
        profiles.insert(
            "org.kde.konsole".into(),
            ProfileConfig {
                format: Format::Shell,
                privacy: Privacy::LocalOnly,
            },
        );
        profiles.insert(
            "1Password".into(),
            ProfileConfig {
                format: Format::CleanProse,
                privacy: Privacy::Disabled,
            },
        );

        // Windows keys profiles by executable name without `.exe` (matched
        // case-insensitively, so "Slack"/"Code"/"1Password" above already
        // cover slack.exe, Code.exe, and 1Password.exe).
        for shell in ["WindowsTerminal", "powershell", "pwsh", "cmd"] {
            profiles.insert(
                shell.into(),
                ProfileConfig {
                    format: Format::Shell,
                    privacy: Privacy::LocalOnly,
                },
            );
        }
        // Password managers on every platform: never listen or paste there.
        for vault in ["KeePassXC", "KeePass", "Bitwarden"] {
            profiles.insert(
                vault.into(),
                ProfileConfig {
                    format: Format::CleanProse,
                    privacy: Privacy::Disabled,
                },
            );
        }

        Self {
            auto_paste: true,
            output_language: "auto".into(),
            input_language: "auto".into(),
            whisper_model: default_whisper_model(),
            llm_model: default_llm_model(),
            custom_dictionary: Vec::new(),
            custom_aliases: HashMap::new(),
            system_message_override: None,
            hotkey: HotkeyConfig::default(),
            llm_cleanup: true,
            sound_cues: true,
            input_device: None,
            profiles,
        }
    }

    /// Profile for an app id (bundle ID, WM_CLASS, or exe name). Exact match
    /// first, then case-insensitive, since Windows exe names and Linux
    /// WM_CLASS values vary in case between apps and versions.
    pub fn profile_for(&self, bundle_id: &str) -> AppProfile {
        if let Some(p) = self.profiles.get(bundle_id) {
            return p.into();
        }
        if let Some((_, p)) = self
            .profiles
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(bundle_id))
        {
            return p.into();
        }
        if let Some(p) = self.profiles.get("default") {
            return p.into();
        }
        AppProfile::default()
    }

    pub fn from_yaml(yaml: &str) -> Result<Self> {
        let mut settings: Settings = serde_yaml::from_str(yaml)?;
        if !settings.profiles.contains_key("default") {
            return Err(VoxError::Config(
                "settings profiles must contain a `default` key".into(),
            ));
        }
        settings.sanitize();
        Ok(settings)
    }

    pub fn from_yaml_checked(yaml: &str) -> Result<(Self, bool)> {
        let mut settings: Settings = serde_yaml::from_str(yaml)?;
        if !settings.profiles.contains_key("default") {
            return Err(VoxError::Config(
                "settings profiles must contain a `default` key".into(),
            ));
        }
        let before = settings.clone();
        settings.sanitize();
        let migrated = before != settings;
        Ok((settings, migrated))
    }

    pub fn sanitize(&mut self) {
        if is_blocked_llm_model(&self.llm_model) {
            self.llm_model = default_llm_model();
        }

        let whisper = self.whisper_model.trim().trim_matches('"');
        if whisper == "models/ggml-small.bin" {
            self.whisper_model = default_whisper_model();
        }

        self.custom_dictionary = sanitize_dictionary(&self.custom_dictionary);
        self.custom_aliases = sanitize_aliases(&self.custom_aliases);

        if self
            .input_device
            .as_deref()
            .is_some_and(|d| d.trim().is_empty() || d.eq_ignore_ascii_case("default"))
        {
            self.input_device = None;
        }

        if let Some(ref override_msg) = self.system_message_override {
            let trimmed = override_msg.trim();
            if trimmed.is_empty() {
                self.system_message_override = None;
            } else if trimmed != override_msg.as_str() {
                self.system_message_override = Some(trimmed.to_string());
            }
        }

        if self.hotkey.enabled {
            let has_primary = self.hotkey.modifiers.iter().any(|m| {
                matches!(
                    m.to_ascii_lowercase().as_str(),
                    "control" | "ctrl" | "alt" | "option" | "super" | "cmd" | "command" | "win"
                        | "windows" | "meta"
                )
            });
            if self.hotkey.modifiers.is_empty() || !has_primary {
                self.hotkey.modifiers = HotkeyConfig::default().modifiers;
            }
            // Store the canonical key name ("." -> "Period") so what's on
            // disk is exactly what gets registered; anything unparseable
            // falls back to the default instead of binding nothing.
            self.hotkey.key = crate::hotkey::canonical_key_name(&self.hotkey.key)
                .unwrap_or_else(|| DEFAULT_HOTKEY_KEY.into());
        }
    }

    pub fn load() -> Self {
        let path = settings_path();
        match fs::read_to_string(&path) {
            Ok(contents) => match Self::from_yaml_checked(&contents) {
                Ok((settings, migrated)) => {
                    if migrated {
                        let _ = settings.save();
                    }
                    settings
                }
                Err(err) => {
                    eprintln!("warning: failed to parse settings ({}): {err}; using defaults", path.display());
                    Self::with_defaults()
                }
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                let settings = Self::with_defaults();
                let _ = settings.save();
                settings
            }
            Err(err) => {
                eprintln!("warning: failed to read settings ({}): {err}; using defaults", path.display());
                Self::with_defaults()
            }
        }
    }

    pub fn save(&self) -> Result<()> {
        let mut sanitized = self.clone();
        sanitized.sanitize();

        let path = settings_path();
        if let Some(parent) = path.parent() {
            secure_fs::ensure_private_dir(parent)?;
        }
        let yaml = serde_yaml::to_string(&sanitized)
            .map_err(|e| VoxError::Config(e.to_string()))?;
        secure_fs::write_private_file(&path, yaml)?;
        Ok(())
    }

    pub fn to_yaml(&self) -> Result<String> {
        serde_yaml::to_string(self).map_err(|e| VoxError::Config(e.to_string()))
    }
}

fn sanitize_string(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let trimmed = cleaned.trim();
    if trimmed.chars().count() > 80 {
        trimmed.chars().take(80).collect()
    } else {
        trimmed.to_string()
    }
}

fn sanitize_dictionary(entries: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for entry in entries {
        let s = sanitize_string(entry);
        if s.is_empty() {
            continue;
        }
        let key = s.to_ascii_lowercase();
        if seen.insert(key) {
            out.push(s);
        }
        if out.len() >= 200 {
            break;
        }
    }
    out
}

fn sanitize_aliases(aliases: &HashMap<String, Vec<String>>) -> HashMap<String, Vec<String>> {
    let mut out = HashMap::new();
    for (key, values) in aliases {
        let canon = sanitize_string(key);
        if canon.is_empty() {
            continue;
        }
        let mut cleaned = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for v in values {
            let alias = sanitize_string(v);
            if alias.is_empty() {
                continue;
            }
            if alias.eq_ignore_ascii_case(&canon) {
                continue;
            }
            let k = alias.to_ascii_lowercase();
            if seen.insert(k) {
                cleaned.push(alias);
            }
        }
        if !cleaned.is_empty() {
            out.insert(canon, cleaned);
        }
    }
    out
}

pub fn settings_mtime(path: &Path) -> Option<std::time::SystemTime> {
    fs::metadata(path).and_then(|m| m.modified()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn privacy_typo_is_hard_error() {
        let yaml = r#"
auto_paste: true
output_language: auto
input_language: auto
whisper_model: /tmp/m.bin
llm_model: llama3.2:latest
profiles:
  default:
    format: clean_prose
    privacy: localy_only
"#;
        let err = Settings::from_yaml(yaml).unwrap_err();
        assert!(err.to_string().contains("privacy") || err.to_string().contains("localy"));
    }

    #[test]
    fn missing_default_profile_errors() {
        let yaml = r#"
profiles:
  com.apple.Terminal:
    format: shell
"#;
        assert!(Settings::from_yaml(yaml).is_err());
    }

    #[test]
    fn sanitize_blocks_qwen_and_is_idempotent() {
        let mut s = Settings::with_defaults();
        s.llm_model = "qwen2.5:latest".into();
        s.sanitize();
        assert_eq!(s.llm_model, "llama3.2:latest");
        let yaml = s.to_yaml().unwrap();
        let (again, migrated) = Settings::from_yaml_checked(&yaml).unwrap();
        assert!(!migrated);
        assert_eq!(again.llm_model, "llama3.2:latest");
    }

    #[test]
    fn aliases_do_not_false_migrate() {
        let mut s = Settings::with_defaults();
        s.custom_dictionary = vec!["Kubernetes".into()];
        s.custom_aliases
            .insert("Kubernetes".into(), vec!["coopernetes".into()]);
        s.sanitize();
        let yaml = s.to_yaml().unwrap();
        let (_again, migrated) = Settings::from_yaml_checked(&yaml).unwrap();
        assert!(!migrated, "canonical settings with aliases must not report migration");
    }

    #[test]
    fn hotkey_sanitize_requires_primary_modifier() {
        let mut s = Settings::with_defaults();
        s.hotkey.modifiers = vec!["shift".into()];
        s.hotkey.key = "".into();
        s.sanitize();
        assert_eq!(s.hotkey.modifiers, vec!["control"]);
        assert_eq!(s.hotkey.key, "Period");
    }

    #[test]
    fn default_hotkey_is_ctrl_period_hybrid() {
        let s = Settings::with_defaults();
        assert_eq!(s.hotkey.modifiers, vec!["control"]);
        assert_eq!(s.hotkey.key, "Period");
        assert_eq!(s.hotkey.mode, HotkeyMode::Hybrid);
        assert!(crate::hotkey::hotkey_from_config(&s.hotkey).is_ok());
    }

    #[test]
    fn hotkey_key_is_canonicalized() {
        let mut s = Settings::with_defaults();
        s.hotkey.key = ".".into();
        s.sanitize();
        assert_eq!(s.hotkey.key, "Period");
        s.hotkey.key = "not a key".into();
        s.sanitize();
        assert_eq!(s.hotkey.key, "Period");
        // existing users' Ctrl+Alt+Space survives untouched
        s.hotkey.modifiers = vec!["control".into(), "alt".into()];
        s.hotkey.key = "Space".into();
        s.sanitize();
        assert_eq!(s.hotkey.key, "Space");
        assert_eq!(s.hotkey.modifiers, vec!["control", "alt"]);
    }

    #[test]
    fn old_settings_without_new_fields_still_load() {
        let yaml = r#"
auto_paste: true
whisper_model: /tmp/m.bin
llm_model: llama3.2:latest
hotkey:
  enabled: true
  modifiers: [control, alt]
  key: Space
profiles:
  default:
    format: clean_prose
    privacy: local_only
"#;
        let s = Settings::from_yaml(yaml).unwrap();
        assert_eq!(s.hotkey.mode, HotkeyMode::Hybrid);
        assert!(s.llm_cleanup);
        assert!(s.sound_cues);
    }

    #[test]
    fn profile_lookup_is_case_insensitive() {
        let s = Settings::with_defaults();
        assert_eq!(s.profile_for("slack").format, Format::Casual);
        assert_eq!(s.profile_for("code").format, Format::CodeContext);
        assert_eq!(s.profile_for("WINDOWSTERMINAL").format, Format::Shell);
        assert_eq!(s.profile_for("keepassxc").privacy, Privacy::Disabled);
        assert_eq!(s.profile_for("notepad").format, Format::CleanProse);
    }

    #[test]
    fn disabled_hotkey_left_alone() {
        let mut s = Settings::with_defaults();
        s.hotkey.enabled = false;
        s.hotkey.modifiers = vec![];
        s.hotkey.key = "".into();
        s.sanitize();
        assert!(s.hotkey.modifiers.is_empty());
        assert!(s.hotkey.key.is_empty());
    }
}
