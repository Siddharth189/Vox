use std::path::Path;

use windows_sys::Win32::Foundation::{CloseHandle, HWND};
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
};

use crate::config::Settings;
use crate::context::AppDetector;
use crate::error::Result;
use crate::model::AppContext;

/// Foreground app via Win32: the foreground window's owning process image
/// name (`Code.exe` -> `Code`) is the profile key, the way a bundle ID is on
/// macOS. Like Linux, anything that can't be resolved (lock screen, a
/// protected process) falls back to the `default` profile rather than
/// failing the dictation.
pub struct WindowsAppDetector {
    settings: Settings,
}

impl WindowsAppDetector {
    pub fn new(settings: Settings) -> Self {
        Self { settings }
    }

    pub fn set_settings(&mut self, settings: Settings) {
        self.settings = settings;
    }
}

impl AppDetector for WindowsAppDetector {
    fn current(&self) -> Result<AppContext> {
        let hwnd = unsafe { GetForegroundWindow() };
        let exe = if hwnd.is_null() {
            None
        } else {
            process_name(hwnd)
        };
        let Some(exe) = exe else {
            return Ok(AppContext {
                bundle_id: "default".into(),
                app_name: "Unknown".into(),
                window_title: None,
                profile: self.settings.profile_for("default"),
            });
        };
        let profile = self.settings.profile_for(&exe);
        Ok(AppContext {
            bundle_id: exe.clone(),
            app_name: exe,
            window_title: window_title(hwnd),
            profile,
        })
    }
}

fn process_name(hwnd: HWND) -> Option<String> {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    if pid == 0 {
        return None;
    }
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return None;
    }
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    let ok = unsafe {
        QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len)
    };
    unsafe { CloseHandle(handle) };
    if ok == 0 || len == 0 {
        return None;
    }
    let full = String::from_utf16_lossy(&buf[..len as usize]);
    exe_stem(&full)
}

/// `C:\...\Code.exe` -> `Code`.
fn exe_stem(path: &str) -> Option<String> {
    Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
}

fn window_title(hwnd: HWND) -> Option<String> {
    let len = unsafe { GetWindowTextLengthW(hwnd) };
    if len <= 0 {
        return None;
    }
    let mut buf = vec![0u16; len as usize + 1];
    let copied = unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    if copied <= 0 {
        return None;
    }
    let title = String::from_utf16_lossy(&buf[..copied as usize]);
    let title = title.trim();
    (!title.is_empty()).then(|| title.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exe_stem_strips_dir_and_extension() {
        assert_eq!(
            exe_stem(r"C:\Users\me\AppData\Local\Programs\Microsoft VS Code\Code.exe").as_deref(),
            Some("Code")
        );
        assert_eq!(
            exe_stem(r"C:\Windows\System32\notepad.exe").as_deref(),
            Some("notepad")
        );
        assert_eq!(exe_stem(""), None);
    }

    #[test]
    fn detector_never_errors() {
        // Whatever is (or isn't) in the foreground while tests run, detection
        // must resolve to some profile instead of failing the dictation.
        let detector = WindowsAppDetector::new(Settings::with_defaults());
        let ctx = detector.current().unwrap();
        assert!(!ctx.bundle_id.is_empty());
    }
}
