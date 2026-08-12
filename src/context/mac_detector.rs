use objc2::MainThreadMarker;
use objc2_app_kit::{NSRunningApplication, NSWorkspace};

use crate::config::Settings;
use crate::context::AppDetector;
use crate::error::{Result, VoxError};
use crate::model::AppContext;

pub struct MacAppDetector {
    settings: Settings,
}

impl MacAppDetector {
    pub fn new(settings: Settings) -> Self {
        Self { settings }
    }

    pub fn set_settings(&mut self, settings: Settings) {
        self.settings = settings;
    }
}

impl AppDetector for MacAppDetector {
    fn current(&self) -> Result<AppContext> {
        // AppKit requires main thread; caller (tray) must invoke from main.
        let _mtm = MainThreadMarker::new().ok_or_else(|| {
            VoxError::Detection("NSWorkspace must be queried on the main thread".into())
        })?;

        let workspace = NSWorkspace::sharedWorkspace();
        let app = workspace.frontmostApplication();

        let app = app.ok_or_else(|| VoxError::Detection("no frontmost application".into()))?;

        let bundle_id = require_bundle_id(&app)?;
        let app_name = app
            .localizedName()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "Unknown".into());

        let profile = self.settings.profile_for(&bundle_id);

        Ok(AppContext {
            bundle_id,
            app_name,
            window_title: None,
            profile,
        })
    }
}

fn require_bundle_id(app: &NSRunningApplication) -> Result<String> {
    let Some(bid) = app.bundleIdentifier() else {
        return Err(VoxError::Detection(
            "frontmost app has no bundle identifier".into(),
        ));
    };
    let s = bid.to_string();
    if s.trim().is_empty() {
        return Err(VoxError::Detection(
            "frontmost app has blank bundle identifier".into(),
        ));
    }
    Ok(s)
}
