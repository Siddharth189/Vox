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
            Some(home) => PathBuf::from(home)
                .join("AppData")
                .join("Local")
                .join("Vox"),
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
/// Edit by voice: the dictation hotkey plus Shift.
pub const DEFAULT_EDIT_HOTKEY_MODIFIERS: &[&str] = &["control", "shift"];

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            modifiers: DEFAULT_HOTKEY_MODIFIERS
                .iter()
                .map(|m| m.to_string())
                .collect(),
            key: DEFAULT_HOTKEY_KEY.into(),
            mode: HotkeyMode::Hybrid,
        }
    }
}

impl HotkeyConfig {
    /// The edit-by-voice default: Ctrl+Shift+. (the dictation hotkey plus Shift).
    pub fn default_edit() -> Self {
        Self {
            modifiers: DEFAULT_EDIT_HOTKEY_MODIFIERS
                .iter()
                .map(|m| m.to_string())
                .collect(),
            ..Self::default()
        }
    }

    /// Without a primary modifier (Ctrl, Alt, Cmd, Win) a global hotkey would
    /// swallow ordinary typing, so one is required; `fallback` supplies it.
    /// The key is stored by its canonical name ("." -> "Period") so what's
    /// on disk is exactly what gets registered.
    fn sanitize(&mut self, fallback: &HotkeyConfig) {
        if !self.enabled {
            return;
        }
        let has_primary = self.modifiers.iter().any(|m| {
            matches!(
                m.to_ascii_lowercase().as_str(),
                "control"
                    | "ctrl"
                    | "alt"
                    | "option"
                    | "super"
                    | "cmd"
                    | "command"
                    | "win"
                    | "windows"
                    | "meta"
            )
        });
        if self.modifiers.is_empty() || !has_primary {
            self.modifiers = fallback.modifiers.clone();
        }
        self.key =
            crate::hotkey::canonical_key_name(&self.key).unwrap_or_else(|| fallback.key.clone());
    }

    /// Same key and modifiers, ignoring order, case, and aliases like "ctrl".
    pub fn same_keys(&self, other: &HotkeyConfig) -> bool {
        fn mods(cfg: &HotkeyConfig) -> Vec<String> {
            let mut m: Vec<String> = cfg
                .modifiers
                .iter()
                .map(|m| match m.to_ascii_lowercase().as_str() {
                    "ctrl" => "control".to_string(),
                    "option" => "alt".to_string(),
                    "cmd" | "command" | "win" | "windows" | "meta" => "super".to_string(),
                    other => other.to_string(),
                })
                .collect();
            m.sort();
            m.dedup();
            m
        }
        let key = |cfg: &HotkeyConfig| {
            crate::hotkey::canonical_key_name(&cfg.key).unwrap_or_else(|| cfg.key.clone())
        };
        mods(self) == mods(other) && key(self) == key(other)
    }
}

/// Which local LLM server polishes dictations when `llm_cleanup` is on.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum LlmProvider {
    /// Ollama's native API (`/api/chat`).
    #[default]
    Ollama,
    /// Any server speaking the OpenAI chat-completions API: LM Studio,
    /// llama.cpp's `llama-server`, Jan, vLLM, LocalAI...
    OpenaiCompatible,
}

impl LlmProvider {
    pub fn default_base_url(self) -> &'static str {
        match self {
            LlmProvider::Ollama => "http://127.0.0.1:11434",
            // LM Studio's default; llama-server users usually set :8080.
            LlmProvider::OpenaiCompatible => "http://127.0.0.1:1234",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            LlmProvider::Ollama => "Ollama",
            LlmProvider::OpenaiCompatible => "OpenAI-compatible server",
        }
    }
}

/// A voice shortcut: saying `trigger` inserts `text` verbatim (an email
/// signature, an address, a link) instead of the spoken words.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Snippet {
    pub trigger: String,
    pub text: String,
}

pub const MAX_SNIPPETS: usize = 100;
const MAX_SNIPPET_CHARS: usize = 4000;

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
    /// Edit by voice: select text, hold this, and say how to change it
    /// ("make it shorter", "reply politely"); the LLM rewrites the selection.
    #[serde(default = "HotkeyConfig::default_edit")]
    pub edit_hotkey: HotkeyConfig,
    /// A small window that shows the words as you speak, with a level meter.
    #[serde(default = "default_true")]
    pub live_text: bool,
    /// Polish text with the local Ollama model when it's running. When it
    /// isn't (or this is off), Vox uses its built-in rule-based cleanup, which
    /// adds no latency and needs no extra install.
    #[serde(default = "default_true")]
    pub llm_cleanup: bool,
    /// Short start/stop tones so you know when Vox is listening without
    /// looking at the tray.
    #[serde(default = "default_true")]
    pub sound_cues: bool,
    /// Transcribe in segments while you're still talking, so stopping only
    /// waits for the last few seconds instead of the whole dictation.
    #[serde(default = "default_true")]
    pub streaming_transcription: bool,
    /// Microphone to record from (case-insensitive name match). None, or a
    /// name that isn't connected right now, means the system default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_device: Option<String>,
    /// Local LLM server API. Ollama by default.
    #[serde(default)]
    pub llm_provider: LlmProvider,
    /// Server address; None means the provider's default local port. Must be
    /// a loopback address unless VOX_ALLOW_REMOTE_LLM=1 is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm_base_url: Option<String>,
    /// Spoken "new line", "new paragraph", and "scratch that".
    #[serde(default = "default_true")]
    pub voice_commands: bool,
    #[serde(default)]
    pub snippets: Vec<Snippet>,
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

/// A UTF-8 byte-order mark, which PowerShell 5's `Set-Content -Encoding
/// utf8` and older Notepad write and the YAML parser rejects.
fn strip_bom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
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
            edit_hotkey: HotkeyConfig::default_edit(),
            live_text: true,
            llm_cleanup: true,
            sound_cues: true,
            streaming_transcription: true,
            input_device: None,
            llm_provider: LlmProvider::Ollama,
            llm_base_url: None,
            voice_commands: true,
            snippets: Vec::new(),
            profiles,
        }
    }

    /// The LLM server address actually used: the configured one, or the
    /// provider's default local port.
    pub fn llm_base_url(&self) -> String {
        self.llm_base_url
            .clone()
            .unwrap_or_else(|| self.llm_provider.default_base_url().to_string())
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
        let mut settings: Settings = serde_yaml::from_str(strip_bom(yaml))?;
        if !settings.profiles.contains_key("default") {
            return Err(VoxError::Config(
                "settings profiles must contain a `default` key".into(),
            ));
        }
        settings.sanitize();
        Ok(settings)
    }

    pub fn from_yaml_checked(yaml: &str) -> Result<(Self, bool)> {
        let mut settings: Settings = serde_yaml::from_str(strip_bom(yaml))?;
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
        self.snippets = sanitize_snippets(&self.snippets);

        if let Some(url) = self.llm_base_url.as_deref() {
            let trimmed = url.trim().trim_end_matches('/');
            self.llm_base_url = if trimmed.is_empty() {
                None
            } else if trimmed != url {
                Some(trimmed.to_string())
            } else {
                self.llm_base_url.take()
            };
        }

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

        self.hotkey.sanitize(&HotkeyConfig::default());
        self.edit_hotkey.sanitize(&HotkeyConfig::default_edit());
        // One key can't do both; dictation wins.
        if self.edit_hotkey.enabled && self.edit_hotkey.same_keys(&self.hotkey) {
            self.edit_hotkey.enabled = false;
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
                    // Also to the log: the windowless tray has no stderr,
                    // and a silent reset to defaults is baffling.
                    tracing::warn!(
                        "failed to parse settings ({}): {err}; using defaults",
                        path.display()
                    );
                    eprintln!(
                        "warning: failed to parse settings ({}): {err}; using defaults",
                        path.display()
                    );
                    Self::with_defaults()
                }
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                let settings = Self::with_defaults();
                let _ = settings.save();
                settings
            }
            Err(err) => {
                eprintln!(
                    "warning: failed to read settings ({}): {err}; using defaults",
                    path.display()
                );
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
        let yaml =
            serde_yaml::to_string(&sanitized).map_err(|e| VoxError::Config(e.to_string()))?;
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

/// Lowercased alphanumeric core of each word: how triggers are compared
/// against speech, so "My email." matches the trigger "my email".
pub fn trigger_words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|w| {
            w.chars()
                .filter(|c| c.is_alphanumeric())
                .flat_map(char::to_lowercase)
                .collect::<String>()
        })
        .filter(|w| !w.is_empty())
        .collect()
}

fn sanitize_snippets(snippets: &[Snippet]) -> Vec<Snippet> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for snippet in snippets {
        let trigger = sanitize_string(&snippet.trigger);
        let words = trigger_words(&trigger);
        // Snippet text keeps its line breaks and tabs: signatures and
        // addresses are multi-line.
        let text: String = snippet
            .text
            .replace("\r\n", "\n")
            .chars()
            .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
            .take(MAX_SNIPPET_CHARS)
            .collect();
        let text = text.trim().to_string();
        if words.is_empty() || text.is_empty() || !seen.insert(words) {
            continue;
        }
        out.push(Snippet { trigger, text });
        if out.len() >= MAX_SNIPPETS {
            break;
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
        assert!(
            !migrated,
            "canonical settings with aliases must not report migration"
        );
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
        assert!(s.streaming_transcription);
        // Settings from before edit by voice and live text get both on.
        assert_eq!(s.edit_hotkey, HotkeyConfig::default_edit());
        assert!(s.live_text);
    }

    #[test]
    fn settings_saved_with_a_byte_order_mark_still_load() {
        // PowerShell 5's `Set-Content -Encoding utf8` (and older Notepad)
        // put a BOM in front; the file must not silently reset to defaults.
        let yaml = "\u{feff}auto_paste: false\nprofiles:\n  default:\n    format: clean_prose\n    privacy: local_only\n";
        let s = Settings::from_yaml(yaml).unwrap();
        assert!(!s.auto_paste);
        let (s, _) = Settings::from_yaml_checked(yaml).unwrap();
        assert!(!s.auto_paste);
    }

    #[test]
    fn default_edit_hotkey_is_ctrl_shift_period() {
        let s = Settings::with_defaults();
        assert!(s.edit_hotkey.enabled);
        assert_eq!(s.edit_hotkey.modifiers, vec!["control", "shift"]);
        assert_eq!(s.edit_hotkey.key, "Period");
        assert!(!s.edit_hotkey.same_keys(&s.hotkey));
        assert!(crate::hotkey::hotkey_from_config(&s.edit_hotkey).is_ok());
    }

    #[test]
    fn edit_hotkey_that_matches_the_dictation_hotkey_is_turned_off() {
        let mut s = Settings::with_defaults();
        s.edit_hotkey.modifiers = vec!["Ctrl".into()];
        s.edit_hotkey.key = ".".into();
        s.sanitize();
        assert!(!s.edit_hotkey.enabled, "Ctrl+. can't both dictate and edit");
        assert!(s.hotkey.enabled);
    }

    #[test]
    fn edit_hotkey_is_sanitized_like_the_dictation_hotkey() {
        let mut s = Settings::with_defaults();
        s.edit_hotkey.modifiers = vec!["shift".into()];
        s.edit_hotkey.key = "not a key".into();
        s.sanitize();
        // A Shift-only shortcut would swallow typing; fall back to the default.
        assert_eq!(s.edit_hotkey.modifiers, vec!["control", "shift"]);
        assert_eq!(s.edit_hotkey.key, "Period");
    }

    #[test]
    fn same_keys_ignores_order_case_and_aliases() {
        let a = HotkeyConfig {
            modifiers: vec!["Ctrl".into(), "Shift".into()],
            key: ".".into(),
            ..HotkeyConfig::default()
        };
        let b = HotkeyConfig {
            modifiers: vec!["shift".into(), "control".into()],
            key: "Period".into(),
            mode: HotkeyMode::Toggle,
            enabled: false,
        };
        assert!(a.same_keys(&b));
        assert!(!a.same_keys(&HotkeyConfig::default()));
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
    fn snippets_are_cleaned_and_deduplicated() {
        let mut s = Settings::with_defaults();
        s.snippets = vec![
            Snippet {
                trigger: "  My   Email ".into(),
                text: "me@example.com".into(),
            },
            // same trigger words once punctuation and case are ignored
            Snippet {
                trigger: "my email.".into(),
                text: "other@example.com".into(),
            },
            Snippet {
                trigger: "sig".into(),
                text: "Thanks,\r\nSam\u{7}".into(),
            },
            Snippet {
                trigger: "...".into(),
                text: "no words in the trigger".into(),
            },
            Snippet {
                trigger: "empty".into(),
                text: "   ".into(),
            },
        ];
        s.sanitize();
        assert_eq!(
            s.snippets,
            vec![
                Snippet {
                    trigger: "My Email".into(),
                    text: "me@example.com".into(),
                },
                Snippet {
                    trigger: "sig".into(),
                    text: "Thanks,\nSam".into(),
                },
            ]
        );
    }

    #[test]
    fn llm_base_url_defaults_per_provider_and_is_trimmed() {
        let mut s = Settings::with_defaults();
        assert_eq!(s.llm_base_url(), "http://127.0.0.1:11434");
        s.llm_provider = LlmProvider::OpenaiCompatible;
        assert_eq!(s.llm_base_url(), "http://127.0.0.1:1234");
        s.llm_base_url = Some(" http://localhost:8080/v1/ ".into());
        s.sanitize();
        assert_eq!(s.llm_base_url.as_deref(), Some("http://localhost:8080/v1"));
        s.llm_base_url = Some("  ".into());
        s.sanitize();
        assert_eq!(s.llm_base_url, None);
    }

    #[test]
    fn settings_without_new_features_get_their_defaults() {
        let yaml = r#"
profiles:
  default:
    format: clean_prose
"#;
        let s = Settings::from_yaml(yaml).unwrap();
        assert!(s.voice_commands);
        assert!(s.snippets.is_empty());
        assert_eq!(s.llm_provider, LlmProvider::Ollama);
        assert_eq!(s.llm_base_url, None);
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
