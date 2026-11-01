//! Small cross-platform helpers that every platform module needs.

use std::path::{Path, PathBuf};

/// Finds `bin` on `PATH` the way a shell would, without spawning `which`
/// (which doesn't exist on Windows and costs a fork everywhere else). On
/// Windows, `PATHEXT` extensions are tried too, so `which("ollama")` finds
/// `ollama.exe`.
pub fn which(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let exts = executable_extensions();
    for dir in std::env::split_paths(&path) {
        for ext in &exts {
            let candidate = dir.join(format!("{bin}{ext}"));
            if is_executable(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(windows)]
fn executable_extensions() -> Vec<String> {
    let mut exts = vec![String::new()];
    let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".EXE;.CMD;.BAT;.COM".into());
    exts.extend(
        pathext
            .split(';')
            .filter(|e| !e.is_empty())
            .map(|e| e.to_ascii_lowercase()),
    );
    exts
}

#[cfg(not(windows))]
fn executable_extensions() -> Vec<String> {
    vec![String::new()]
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// Opens `url` in the user's default browser.
pub fn open_url(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        expect_success(std::process::Command::new("open").arg(url).status()?)
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::Shell::ShellExecuteW;
        use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

        let op = wide("open");
        let file = wide(url);
        // ShellExecute hands the URL to the registered browser without a
        // console window (spawning `cmd /c start` would flash one from the
        // windowless tray). Return values above 32 mean success.
        let rc = unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                op.as_ptr(),
                file.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                SW_SHOWNORMAL,
            )
        };
        if rc as isize > 32 {
            Ok(())
        } else {
            Err(std::io::Error::other(format!(
                "ShellExecuteW failed ({})",
                rc as isize
            )))
        }
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        expect_success(std::process::Command::new("xdg-open").arg(url).status()?)
    }
}

#[cfg(not(windows))]
fn expect_success(status: std::process::ExitStatus) -> std::io::Result<()> {
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "opener exited with {status}"
        )))
    }
}

/// NUL-terminated UTF-16, for Win32 `W` APIs.
#[cfg(windows)]
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn which_finds_nothing_for_nonsense() {
        assert!(which("definitely-not-a-real-binary-vox-test").is_none());
    }

    #[test]
    fn which_finds_a_system_binary() {
        #[cfg(windows)]
        let bin = "cmd";
        #[cfg(not(windows))]
        let bin = "sh";
        assert!(which(bin).is_some(), "{bin} should be on PATH");
    }
}
