use std::path::Path;
use std::process::Command;
use std::time::Duration;

use serde::Serialize;

use crate::config::{self, Settings};
use crate::permissions;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
    Info,
}

#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub name: String,
    pub status: CheckStatus,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DoctorReport {
    pub checks: Vec<Check>,
    pub pass: usize,
    pub warn: usize,
    pub fail: usize,
    pub info: usize,
}

impl DoctorReport {
    fn from_checks(checks: Vec<Check>) -> Self {
        let mut pass = 0;
        let mut warn = 0;
        let mut fail = 0;
        let mut info = 0;
        for c in &checks {
            match c.status {
                CheckStatus::Pass => pass += 1,
                CheckStatus::Warn => warn += 1,
                CheckStatus::Fail => fail += 1,
                CheckStatus::Info => info += 1,
            }
        }
        Self {
            checks,
            pass,
            warn,
            fail,
            info,
        }
    }

    pub fn print_human(&self) {
        for c in &self.checks {
            let tag = match c.status {
                CheckStatus::Pass => "PASS",
                CheckStatus::Warn => "WARN",
                CheckStatus::Fail => "FAIL",
                CheckStatus::Info => "INFO",
            };
            println!("[{tag}] {}: {}", c.name, c.message);
            if let Some(fix) = &c.fix {
                println!("       fix: {fix}");
            }
        }
        println!(
            "\nSummary: {} pass, {} warn, {} fail, {} info",
            self.pass, self.warn, self.fail, self.info
        );
    }
}

pub fn run(settings: &Settings) -> DoctorReport {
    let mut checks = Vec::new();

    // 1. macOS
    #[cfg(target_os = "macos")]
    checks.push(Check {
        name: "macos".into(),
        status: CheckStatus::Pass,
        message: "running on macOS".into(),
        fix: None,
    });
    #[cfg(not(target_os = "macos"))]
    checks.push(Check {
        name: "macos".into(),
        status: CheckStatus::Fail,
        message: "Vox tray/hotkey/paste are macOS-only".into(),
        fix: Some("Build and run on macOS".into()),
    });

    // 2. Apple Silicon
    #[cfg(target_arch = "aarch64")]
    checks.push(Check {
        name: "arch".into(),
        status: CheckStatus::Pass,
        message: "Apple Silicon (aarch64)".into(),
        fix: None,
    });
    #[cfg(not(target_arch = "aarch64"))]
    checks.push(Check {
        name: "arch".into(),
        status: CheckStatus::Warn,
        message: format!("architecture {} (Apple Silicon recommended)", std::env::consts::ARCH),
        fix: None,
    });

    // 3. ollama CLI
    if which("ollama") {
        checks.push(Check {
            name: "ollama_cli".into(),
            status: CheckStatus::Pass,
            message: "ollama on PATH".into(),
            fix: None,
        });
    } else {
        checks.push(Check {
            name: "ollama_cli".into(),
            status: CheckStatus::Warn,
            message: "ollama not found on PATH".into(),
            fix: Some("brew install ollama".into()),
        });
    }

    // 4. cmake
    if which("cmake") {
        checks.push(Check {
            name: "cmake".into(),
            status: CheckStatus::Pass,
            message: "cmake on PATH".into(),
            fix: None,
        });
    } else {
        checks.push(Check {
            name: "cmake".into(),
            status: CheckStatus::Warn,
            message: "cmake not found (needed to build whisper-rs)".into(),
            fix: Some("brew install cmake".into()),
        });
    }

    // 5. Xcode CLT
    let xcode = Command::new("xcode-select").arg("-p").output();
    match xcode {
        Ok(out) if out.status.success() => checks.push(Check {
            name: "xcode_clt".into(),
            status: CheckStatus::Pass,
            message: "Xcode Command Line Tools present".into(),
            fix: None,
        }),
        _ => checks.push(Check {
            name: "xcode_clt".into(),
            status: CheckStatus::Warn,
            message: "Xcode Command Line Tools not found".into(),
            fix: Some("xcode-select --install".into()),
        }),
    }

    // 6. Settings file
    let settings_path = config::settings_path();
    if settings_path.exists() {
        checks.push(Check {
            name: "settings".into(),
            status: CheckStatus::Pass,
            message: format!("settings at {}", settings_path.display()),
            fix: None,
        });
    } else {
        let parent = settings_path.parent().unwrap_or(Path::new("."));
        match std::fs::create_dir_all(parent) {
            Ok(()) => checks.push(Check {
                name: "settings".into(),
                status: CheckStatus::Warn,
                message: "settings file missing (will be created on first run)".into(),
                fix: None,
            }),
            Err(e) => checks.push(Check {
                name: "settings".into(),
                status: CheckStatus::Fail,
                message: format!("cannot create settings dir: {e}"),
                fix: None,
            }),
        }
    }

    // 7. Whisper model
    let model_path = Path::new(&settings.whisper_model);
    if !model_path.exists() {
        checks.push(Check {
            name: "whisper_model".into(),
            status: CheckStatus::Fail,
            message: format!("model missing at {}", model_path.display()),
            fix: Some("scripts/download_model.sh".into()),
        });
    } else {
        match std::fs::File::open(model_path).and_then(|mut f| {
            use std::io::Read;
            let mut magic = [0u8; 4];
            f.read_exact(&mut magic)?;
            Ok(magic)
        }) {
            Ok(magic) if magic == [0x6c, 0x6d, 0x67, 0x67] => checks.push(Check {
                name: "whisper_model".into(),
                status: CheckStatus::Pass,
                message: format!("valid ggml model at {}", model_path.display()),
                fix: None,
            }),
            Ok(_) => checks.push(Check {
                name: "whisper_model".into(),
                status: CheckStatus::Warn,
                message: "model file exists but ggml magic mismatch".into(),
                fix: Some("scripts/download_model.sh".into()),
            }),
            Err(e) => checks.push(Check {
                name: "whisper_model".into(),
                status: CheckStatus::Warn,
                message: format!("model unreadable: {e}"),
                fix: None,
            }),
        }
    }

    // 8. Ollama reachability + model
    match check_ollama(&settings.llm_model) {
        OllamaCheck::Ok => checks.push(Check {
            name: "ollama".into(),
            status: CheckStatus::Pass,
            message: format!("ollama reachable; model {} present", settings.llm_model),
            fix: None,
        }),
        OllamaCheck::Unreachable => checks.push(Check {
            name: "ollama".into(),
            status: CheckStatus::Fail,
            message: "ollama unreachable at http://127.0.0.1:11434".into(),
            fix: Some("open -ga Ollama  # or: ollama serve".into()),
        }),
        OllamaCheck::MissingModel => {
            let pull = settings.llm_model.trim_end_matches(":latest");
            checks.push(Check {
                name: "ollama".into(),
                status: CheckStatus::Fail,
                message: format!("model {} not found in ollama tags", settings.llm_model),
                fix: Some(format!("ollama pull {pull}")),
            });
        }
    }

    // 9. Microphone
    checks.push(Check {
        name: "microphone".into(),
        status: CheckStatus::Info,
        message: "microphone permission is prompted on first recording".into(),
        fix: None,
    });

    // 10. Accessibility
    if !settings.auto_paste {
        checks.push(Check {
            name: "accessibility".into(),
            status: CheckStatus::Info,
            message: "auto_paste disabled; accessibility not required".into(),
            fix: None,
        });
    } else if !permissions::running_from_app_bundle() {
        checks.push(Check {
            name: "accessibility".into(),
            status: CheckStatus::Warn,
            message: "not running from Vox.app; CLI doctor cannot speak for the installed app's grant"
                .into(),
            fix: Some("Open Accessibility Settings from the Vox tray menu".into()),
        });
    } else if permissions::accessibility_trusted() {
        checks.push(Check {
            name: "accessibility".into(),
            status: CheckStatus::Pass,
            message: "Accessibility trusted".into(),
            fix: None,
        });
    } else {
        checks.push(Check {
            name: "accessibility".into(),
            status: CheckStatus::Warn,
            message: "Accessibility not trusted (auto-paste may fall back to clipboard)".into(),
            fix: Some("Open Accessibility Settings from the Vox tray menu".into()),
        });
    }

    DoctorReport::from_checks(checks)
}

enum OllamaCheck {
    Ok,
    Unreachable,
    MissingModel,
}

fn check_ollama(model: &str) -> OllamaCheck {
    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
    {
        Ok(c) => c,
        Err(_) => return OllamaCheck::Unreachable,
    };
    let resp = match client.get("http://127.0.0.1:11434/api/tags").send() {
        Ok(r) if r.status().is_success() => r,
        _ => return OllamaCheck::Unreachable,
    };
    let body: serde_json::Value = match resp.json() {
        Ok(v) => v,
        Err(_) => return OllamaCheck::Unreachable,
    };
    let Some(models) = body.get("models").and_then(|m| m.as_array()) else {
        return OllamaCheck::MissingModel;
    };
    let names: Vec<String> = models
        .iter()
        .filter_map(|m| m.get("name").and_then(|n| n.as_str()).map(|s| s.to_string()))
        .collect();
    if names.iter().any(|n| model_matches(n, model)) {
        OllamaCheck::Ok
    } else {
        OllamaCheck::MissingModel
    }
}

pub fn model_matches(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let strip = |s: &str| -> String {
        s.strip_suffix(":latest").unwrap_or(s).to_string()
    };
    strip(a) == strip(b)
}

fn which(bin: &str) -> bool {
    Command::new("which")
        .arg(bin)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
