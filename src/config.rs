use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Result, VoxError};
use crate::model::{AppProfile, Format, Privacy};
use crate::secure_fs;

#[cfg(target_os = "macos")]
pub fn data_dir() -> PathBuf {
    match std::env::var_os("HOME") {
        Some(home) => PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join("vox"),
        None => PathBuf::from("."),
    }
}

/// XDG Base Directory Specification: $XDG_DATA_HOME, falling back to ~/.local/share.
#[cfg(not(target_os = "macos"))]
pub fn data_dir() -> PathBuf {
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HotkeyConfig {
    pub enabled: bool,
    pub modifiers: Vec<String>,
    pub key: String,
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            modifiers: vec!["control".into(), "alt".into()],
            key: "Space".into(),
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
    pub profiles: HashMap<String, ProfileConfig>,
}

fn default_true() -> bool {
    true
}

fn default_auto() -> String {
    "auto".into()
}

fn default_whisper_model() -> String {
    model_dir()
        .join("ggml-small.bin")
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
            profiles,
        }
    }

    pub fn profile_for(&self, bundle_id: &str) -> AppProfile {
        if let Some(p) = self.profiles.get(bundle_id) {
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
                    "control" | "ctrl" | "alt" | "option" | "super" | "cmd" | "command"
                )
            });
            if self.hotkey.modifiers.is_empty() || !has_primary {
                self.hotkey.modifiers = vec!["control".into(), "alt".into()];
            }
            if self.hotkey.key.trim().is_empty() {
                self.hotkey.key = "Space".into();
            }
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
        assert_eq!(s.hotkey.modifiers, vec!["control", "alt"]);
        assert_eq!(s.hotkey.key, "Space");
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
