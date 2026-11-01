#[cfg(not(any(target_os = "macos", windows)))]
mod linux_detector;
#[cfg(target_os = "macos")]
mod mac_detector;
#[cfg(windows)]
mod windows_detector;

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

#[cfg(not(any(target_os = "macos", windows)))]
pub use linux_detector::LinuxAppDetector;
#[cfg(target_os = "macos")]
pub use mac_detector::MacAppDetector;
#[cfg(windows)]
pub use windows_detector::WindowsAppDetector;

/// The real foreground-app detector for this platform.
pub fn platform_detector(settings: &Settings) -> Box<dyn AppDetector> {
    #[cfg(target_os = "macos")]
    {
        Box::new(MacAppDetector::new(settings.clone()))
    }
    #[cfg(windows)]
    {
        Box::new(WindowsAppDetector::new(settings.clone()))
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        Box::new(LinuxAppDetector::new(settings.clone()))
    }
}
