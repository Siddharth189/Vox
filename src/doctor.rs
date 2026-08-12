use std::path::Path;
use std::process::Command;
use std::time::Duration;

use serde::Serialize;

use crate::config::{self, Settings};
#[cfg(target_os = "macos")]
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

    // 1. Supported OS
    #[cfg(target_os = "macos")]
    checks.push(Check {
        name: "os".into(),
        status: CheckStatus::Pass,
        message: "running on macOS".into(),
        fix: None,
    });
    #[cfg(target_os = "linux")]
    checks.push(Check {
        name: "os".into(),
        status: CheckStatus::Pass,
        message: "running on Linux".into(),
        fix: None,
    });
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    checks.push(Check {
        name: "os".into(),
        status: CheckStatus::Fail,
        message: "Vox supports macOS and Linux only".into(),
        fix: Some("Build and run on macOS or Linux".into()),
    });

    // 2. Architecture (informational everywhere except macOS, where Apple
    // Silicon is what's actually tested)
    #[cfg(target_os = "macos")]
    {
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
    }
    #[cfg(not(target_os = "macos"))]
    checks.push(Check {
        name: "arch".into(),
        status: CheckStatus::Info,
        message: format!("architecture {}", std::env::consts::ARCH),
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
            fix: Some(install_hint("ollama")),
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
            fix: Some(install_hint("cmake")),
        });
    }

    // 5. Native build toolchain: Xcode CLT on macOS, a C compiler on Linux
    #[cfg(target_os = "macos")]
    {
        let xcode = Command::new("xcode-select").arg("-p").output();
        match xcode {
            Ok(out) if out.status.success() => checks.push(Check {
                name: "toolchain".into(),
                status: CheckStatus::Pass,
                message: "Xcode Command Line Tools present".into(),
                fix: None,
            }),
            _ => checks.push(Check {
                name: "toolchain".into(),
                status: CheckStatus::Warn,
                message: "Xcode Command Line Tools not found".into(),
                fix: Some("xcode-select --install".into()),
            }),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        if which("cc") || which("gcc") || which("clang") {
            checks.push(Check {
                name: "toolchain".into(),
                status: CheckStatus::Pass,
                message: "C compiler present".into(),
                fix: None,
            });
        } else {
            checks.push(Check {
                name: "toolchain".into(),
                status: CheckStatus::Warn,
                message: "no C compiler found (needed to build whisper-rs)".into(),
                fix: Some(install_hint("gcc")),
            });
        }
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
        OllamaCheck::Ok { model_size_bytes } => {
            checks.push(ollama_model_fit_check(&settings.llm_model, model_size_bytes));
        }
        OllamaCheck::Unreachable => checks.push(Check {
            name: "ollama".into(),
            status: CheckStatus::Fail,
            message: "ollama unreachable at http://127.0.0.1:11434".into(),
            fix: Some(ollama_start_hint()),
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

    // 10. Auto-paste permission: Accessibility (TCC) on macOS, no equivalent
    // gate on Linux beyond having a graphical session for enigo/XTest to use.
    #[cfg(target_os = "macos")]
    {
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
    }
    #[cfg(not(target_os = "macos"))]
    {
        if !settings.auto_paste {
            checks.push(Check {
                name: "input_session".into(),
                status: CheckStatus::Info,
                message: "auto_paste disabled; no graphical input session required".into(),
                fix: None,
            });
        } else if std::env::var_os("DISPLAY").is_some()
            || std::env::var_os("WAYLAND_DISPLAY").is_some()
        {
            checks.push(Check {
                name: "input_session".into(),
                status: CheckStatus::Pass,
                message: "graphical session detected; Linux has no Accessibility-style permission gate, auto-paste synthesizes Ctrl+V directly".into(),
                fix: None,
            });
        } else {
            checks.push(Check {
                name: "input_session".into(),
                status: CheckStatus::Warn,
                message: "no DISPLAY or WAYLAND_DISPLAY set; auto-paste needs a graphical session".into(),
                fix: None,
            });
        }
    }

    // 11. Per-app profile detection (Linux only - macOS always has NSWorkspace)
    #[cfg(not(target_os = "macos"))]
    checks.push(linux_app_detection_check());

    DoctorReport::from_checks(checks)
}

#[cfg(not(target_os = "macos"))]
fn linux_app_detection_check() -> Check {
    let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_default();
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();

    if which("kdotool") {
        return Check {
            name: "app_detection".into(),
            status: CheckStatus::Pass,
            message: "kdotool found; per-app profiles work on KDE Plasma (X11 or Wayland)".into(),
            fix: None,
        };
    }
    if session == "x11" && which("xdotool") {
        return Check {
            name: "app_detection".into(),
            status: CheckStatus::Pass,
            message: "xdotool found on an X11 session; per-app profiles enabled".into(),
            fix: None,
        };
    }
    if desktop.to_ascii_lowercase().contains("kde") {
        return Check {
            name: "app_detection".into(),
            status: CheckStatus::Warn,
            message: "KDE Plasma detected but kdotool is missing; every dictation will use the default profile".into(),
            fix: Some(install_hint("kdotool")),
        };
    }
    if session == "x11" {
        return Check {
            name: "app_detection".into(),
            status: CheckStatus::Warn,
            message: "X11 session but xdotool is missing; every dictation will use the default profile".into(),
            fix: Some(install_hint("xdotool")),
        };
    }
    Check {
        name: "app_detection".into(),
        status: CheckStatus::Info,
        message: format!(
            "no supported active-window query method on this desktop ({desktop}, {session}); every dictation uses the default profile"
        ),
        fix: None,
    }
}

enum OllamaCheck {
    Ok { model_size_bytes: Option<u64> },
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
    let matched = models.iter().find(|m| {
        m.get("name")
            .and_then(|n| n.as_str())
            .is_some_and(|n| model_matches(n, model))
    });
    match matched {
        Some(m) => OllamaCheck::Ok {
            model_size_bytes: m.get("size").and_then(|s| s.as_u64()),
        },
        None => OllamaCheck::MissingModel,
    }
}

/// A model that's large relative to total RAM forces the OS to swap during
/// inference, which doesn't just slow things down - it can push a single
/// dictation's cleanup step past Vox's request timeout entirely. Observed
/// on an 8GB/Linux machine: a ~2GB model took 4+ minutes (and counting) for
/// a trivial prompt once system + model memory exceeded physical RAM,
/// against a 60s timeout; a ~1.3GB model on the same machine responded in
/// single-digit seconds. macOS's unified memory and Metal acceleration
/// change this enough that the same heuristic doesn't apply there.
#[cfg(target_os = "macos")]
fn ollama_model_fit_check(model: &str, _model_size_bytes: Option<u64>) -> Check {
    Check {
        name: "ollama".into(),
        status: CheckStatus::Pass,
        message: format!("ollama reachable; model {model} present"),
        fix: None,
    }
}

// Deliberately not a memory-fit ratio formula: "available RAM" swings wildly
// with whatever else happens to be open when `vox doctor` runs, so a ratio
// against it is unstable, and a ratio against *total* RAM was tried first
// and proved too permissive - it didn't flag the exact case measured below.
// These are the two real data points from an 8GB Fedora/KDE Wayland laptop
// under normal desktop load (browser, editor, the usual): a 1.3GB model
// (llama3.2:1b) responded in single-digit seconds; a 2.0GB model
// (llama3.2:latest, the default) took 4+ minutes and never completed within
// even a 240s timeout, well past Vox's real 60s one. The threshold below is
// the same one process/prompt.rs uses to decide whether to trim the system
// prompt, so there's one consistent "is this a constrained machine" line.
#[cfg(not(target_os = "macos"))]
const LARGE_MODEL_THRESHOLD_BYTES: u64 = 1_500_000_000;

#[cfg(not(target_os = "macos"))]
fn ollama_model_fit_check(model: &str, model_size_bytes: Option<u64>) -> Check {
    let base_message = format!("ollama reachable; model {model} present");
    let (Some(model_bytes), Some(total_ram_bytes)) =
        (model_size_bytes, config::total_ram_bytes())
    else {
        return Check {
            name: "ollama".into(),
            status: CheckStatus::Pass,
            message: base_message,
            fix: None,
        };
    };

    if total_ram_bytes < config::LOW_RAM_THRESHOLD_BYTES && model_bytes > LARGE_MODEL_THRESHOLD_BYTES {
        Check {
            name: "ollama".into(),
            status: CheckStatus::Warn,
            message: format!(
                "{base_message}, but it's {:.1}GB on a {:.1}GB-RAM machine; expect slow or timed-out LLM cleanup under normal desktop load",
                model_bytes as f64 / 1e9,
                total_ram_bytes as f64 / 1e9,
            ),
            fix: Some("try a smaller model, e.g. `ollama pull llama3.2:1b` and set llm_model in settings.yaml".into()),
        }
    } else {
        Check {
            name: "ollama".into(),
            status: CheckStatus::Pass,
            message: base_message,
            fix: None,
        }
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

#[cfg(target_os = "macos")]
fn install_hint(pkg: &str) -> String {
    format!("brew install {pkg}")
}

#[cfg(not(target_os = "macos"))]
fn install_hint(pkg: &str) -> String {
    // Ollama isn't packaged in the major distro repos; the vendor's install
    // script is the documented path everywhere.
    if pkg == "ollama" {
        return "curl -fsSL https://ollama.com/install.sh | sh".into();
    }
    if which("dnf") {
        format!("sudo dnf install {pkg}")
    } else if which("apt") || which("apt-get") {
        format!("sudo apt install {pkg}")
    } else if which("pacman") {
        format!("sudo pacman -S {pkg}")
    } else {
        format!("install {pkg} with your distro's package manager")
    }
}

#[cfg(target_os = "macos")]
fn ollama_start_hint() -> String {
    "open -ga Ollama  # or: ollama serve".into()
}

#[cfg(not(target_os = "macos"))]
fn ollama_start_hint() -> String {
    "ollama serve  # or: systemctl --user start ollama, if you set that up".into()
}
