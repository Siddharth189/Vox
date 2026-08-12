#[cfg(target_os = "macos")]
mod mac_detector;

use crate::config::Settings;
use crate::error::Result;
use crate::model::AppContext;

pub trait AppDetector: Send + Sync {
    fn current(&self) -> Result<AppContext>;
}

pub struct StubAppDetector {
    pub ctx: AppContext,
}

impl StubAppDetector {
    pub fn new(ctx: AppContext) -> Self {
        Self { ctx }
    }

    pub fn with_bundle(bundle_id: &str, app_name: &str, settings: &Settings) -> Self {
        Self {
            ctx: AppContext {
                bundle_id: bundle_id.into(),
                app_name: app_name.into(),
                window_title: None,
                profile: settings.profile_for(bundle_id),
            },
        }
    }
}

impl AppDetector for StubAppDetector {
    fn current(&self) -> Result<AppContext> {
        Ok(self.ctx.clone())
    }
}

#[cfg(target_os = "macos")]
pub use mac_detector::MacAppDetector;
