//! Private-by-default file IO for settings, history, and logs.
//!
//! Unix: files are 0600 and directories 0700. Windows: everything lives under
//! %LOCALAPPDATA%, whose inherited ACL already restricts it to the current
//! user (plus SYSTEM/Administrators), so no extra ACL work is needed.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::Path;

use crate::error::Result;

#[cfg(unix)]
const FILE_MODE: u32 = 0o600;
#[cfg(unix)]
const DIR_MODE: u32 = 0o700;

pub fn ensure_private_dir(path: &Path) -> Result<()> {
    if !path.exists() {
        fs::create_dir_all(path)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(DIR_MODE))?;
    }
    Ok(())
}

fn private_options() -> OpenOptions {
    #[allow(unused_mut)]
    let mut opts = OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(FILE_MODE);
    }
    opts
}

/// Writes `contents` to `path` atomically: readers (the tray polls
/// settings.yaml while the settings UI saves it) see either the old file or
/// the new one, never a truncated half-write.
pub fn write_private_file(path: &Path, contents: impl AsRef<[u8]>) -> Result<()> {
    if let Some(parent) = path.parent() {
        ensure_private_dir(parent)?;
    }

    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "vox".into());
    let tmp = path.with_file_name(format!(".{file_name}.{}.tmp", std::process::id()));

    let result = (|| -> Result<()> {
        let mut file = private_options()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp)?;
        file.write_all(contents.as_ref())?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Opens (creating if needed) a private append-only file, e.g. tray.log.
pub fn open_private_append(path: &Path) -> Result<File> {
    if let Some(parent) = path.parent() {
        ensure_private_dir(parent)?;
    }
    Ok(private_options().create(true).append(true).open(path)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_replaces_and_leaves_no_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.yaml");
        write_private_file(&path, "a: 1").unwrap();
        write_private_file(&path, "a: 2").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "a: 2");
        let names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 1, "temp file left behind: {names:?}");
    }

    #[cfg(unix)]
    #[test]
    fn files_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.json");
        write_private_file(&path, "[]").unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
