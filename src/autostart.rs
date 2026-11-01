//! "Launch at login" for the tray, per platform:
//! Windows: HKCU\Software\Microsoft\Windows\CurrentVersion\Run
//! macOS: ~/Library/LaunchAgents/dev.vox.tray.plist
//! Linux: ~/.config/autostart/vox.desktop (XDG autostart)

use std::path::PathBuf;

use crate::error::{Result, VoxError};

/// The executable that should be started at login: the windowless
/// `vox-tray` next to the current binary when it exists, else this binary
/// with the `tray` argument.
pub fn tray_command() -> Result<(PathBuf, Vec<String>)> {
    let exe = std::env::current_exe()?;
    let dir = exe.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let tray_name = if cfg!(windows) {
        "vox-tray.exe"
    } else {
        "vox-tray"
    };
    let tray = dir.join(tray_name);
    if exe.file_name().map(|n| n == tray_name).unwrap_or(false) {
        return Ok((exe, vec![]));
    }
    if tray.exists() {
        return Ok((tray, vec![]));
    }
    Ok((exe, vec!["tray".into()]))
}

#[cfg(windows)]
mod imp {
    use super::*;
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ,
    };

    use crate::util::wide;

    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const VALUE: &str = "Vox";

    pub fn is_enabled() -> bool {
        let key = wide(RUN_KEY);
        let value = wide(VALUE);
        let mut size = 0u32;
        let rc = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut size,
            )
        };
        rc == ERROR_SUCCESS
    }

    pub fn set_enabled(enabled: bool) -> Result<()> {
        let key = wide(RUN_KEY);
        let value = wide(VALUE);
        if !enabled {
            let rc = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr()) };
            return if rc == ERROR_SUCCESS || !is_enabled() {
                Ok(())
            } else {
                Err(VoxError::Other(format!(
                    "could not remove login item (error {rc})"
                )))
            };
        }
        let (exe, args) = tray_command()?;
        let mut cmd = format!("\"{}\"", exe.display());
        for a in args {
            cmd.push(' ');
            cmd.push_str(&a);
        }
        let data = wide(&cmd);
        let rc = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                value.as_ptr(),
                REG_SZ,
                data.as_ptr().cast(),
                (data.len() * 2) as u32,
            )
        };
        if rc == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(VoxError::Other(format!(
                "could not add login item (error {rc})"
            )))
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;

    fn plist_path() -> Option<PathBuf> {
        std::env::var_os("HOME").map(|h| {
            PathBuf::from(h)
                .join("Library")
                .join("LaunchAgents")
                .join("dev.vox.tray.plist")
        })
    }

    pub fn is_enabled() -> bool {
        plist_path().is_some_and(|p| p.exists())
    }

    pub fn set_enabled(enabled: bool) -> Result<()> {
        let path = plist_path().ok_or_else(|| VoxError::Other("HOME not set".into()))?;
        if !enabled {
            if path.exists() {
                std::fs::remove_file(&path)?;
            }
            return Ok(());
        }
        let (exe, args) = tray_command()?;
        let mut program_args = format!(
            "    <string>{}</string>\n",
            xml_escape(&exe.to_string_lossy())
        );
        for a in args {
            program_args.push_str(&format!("    <string>{}</string>\n", xml_escape(&a)));
        }
        let plist = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n<dict>\n  <key>Label</key>\n  <string>dev.vox.tray</string>\n\
  <key>ProgramArguments</key>\n  <array>\n{program_args}  </array>\n\
  <key>RunAtLoad</key>\n  <true/>\n  <key>ProcessType</key>\n  <string>Interactive</string>\n</dict>\n</plist>\n"
        );
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, plist)?;
        Ok(())
    }

    fn xml_escape(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod imp {
    use super::*;

    fn desktop_path() -> Option<PathBuf> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        Some(base.join("autostart").join("vox.desktop"))
    }

    pub fn is_enabled() -> bool {
        desktop_path().is_some_and(|p| p.exists())
    }

    pub fn set_enabled(enabled: bool) -> Result<()> {
        let path = desktop_path().ok_or_else(|| VoxError::Other("HOME not set".into()))?;
        if !enabled {
            if path.exists() {
                std::fs::remove_file(&path)?;
            }
            return Ok(());
        }
        let (exe, args) = tray_command()?;
        let mut exec = format!("\"{}\"", exe.display());
        for a in args {
            exec.push(' ');
            exec.push_str(&a);
        }
        let desktop = format!(
            "[Desktop Entry]\nType=Application\nName=Vox\nComment=Local voice dictation\nExec={exec}\nTerminal=false\nX-GNOME-Autostart-enabled=true\n"
        );
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, desktop)?;
        Ok(())
    }
}

pub use imp::{is_enabled, set_enabled};
