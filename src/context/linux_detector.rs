use std::process::Command;

use crate::config::Settings;
use crate::context::AppDetector;
use crate::error::Result;
use crate::model::AppContext;

/// Linux has no single API for "the focused app" the way macOS has
/// NSWorkspace. What's queryable depends on the session:
///
/// - KDE Plasma (X11 or Wayland): `kdotool`, which drives KWin's scripting
///   D-Bus interface. This is the only reliable method on Plasma Wayland;
///   XWayland's `_NET_ACTIVE_WINDOW` points at a KWin-internal focus proxy
///   window there, not the real client, so plain `xdotool`/`xprop` return
///   garbage (empty title, no PID) even though they don't error.
/// - Other X11 sessions: `xdotool` against the real X server.
/// - Everything else (GNOME Wayland, Sway, Hyprland, ...): there is no
///   portable way to query the focused window without a compositor-specific
///   extension. Detection falls back to the `default` profile rather than
///   failing the dictation - unlike macOS's fail-closed behavior for a
///   *missing* frontmost app, this is a *platform capability* gap, and
///   failing every dictation because of it would make Vox unusable there.
pub struct LinuxAppDetector {
    settings: Settings,
}

impl LinuxAppDetector {
    pub fn new(settings: Settings) -> Self {
        Self { settings }
    }

    pub fn set_settings(&mut self, settings: Settings) {
        self.settings = settings;
    }
}

impl AppDetector for LinuxAppDetector {
    fn current(&self) -> Result<AppContext> {
        let window = active_window();

        let Some(window) = window else {
            return Ok(AppContext {
                bundle_id: "default".into(),
                app_name: "Unknown".into(),
                window_title: None,
                profile: self.settings.profile_for("default"),
            });
        };

        let profile = self.settings.profile_for(&window.class);
        Ok(AppContext {
            bundle_id: window.class.clone(),
            app_name: if window.class.is_empty() {
                "Unknown".into()
            } else {
                window.class
            },
            window_title: window.title,
            profile,
        })
    }
}

struct ActiveWindow {
    /// WM_CLASS-derived identifier. Used the same way a macOS bundle ID is:
    /// as the key into per-app profiles.
    class: String,
    title: Option<String>,
}

fn active_window() -> Option<ActiveWindow> {
    if which("kdotool") {
        if let Some(w) = kdotool_active_window() {
            return Some(w);
        }
    }
    if std::env::var("XDG_SESSION_TYPE").as_deref() == Ok("x11") && which("xdotool") {
        if let Some(w) = xdotool_active_window() {
            return Some(w);
        }
    }
    None
}

fn kdotool_active_window() -> Option<ActiveWindow> {
    let id = run("kdotool", &["getactivewindow"])?;
    let id = id.trim();
    if id.is_empty() {
        return None;
    }
    let class = run("kdotool", &["getwindowclassname", id]).unwrap_or_default();
    let title = run("kdotool", &["getwindowname", id]);
    let class = class.trim().to_string();
    if class.is_empty() {
        return None;
    }
    Some(ActiveWindow {
        class,
        title: title.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()),
    })
}

fn xdotool_active_window() -> Option<ActiveWindow> {
    let id = run("xdotool", &["getactivewindow"])?;
    let id = id.trim();
    if id.is_empty() {
        return None;
    }
    let class = run("xdotool", &["getwindowclassname", id]).unwrap_or_default();
    let title = run("xdotool", &["getwindowname", id]);
    let class = class.trim().to_string();
    if class.is_empty() {
        return None;
    }
    Some(ActiveWindow {
        class,
        title: title.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()),
    })
}

fn run(bin: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(bin).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout).ok()
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
