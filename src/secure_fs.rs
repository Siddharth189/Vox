use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

use crate::error::{Result, VoxError};

const FILE_MODE: u32 = 0o600;
const DIR_MODE: u32 = 0o700;

pub fn ensure_private_dir(path: &Path) -> Result<()> {
    if !path.exists() {
        fs::create_dir_all(path)?;
    }
    let perms = fs::Permissions::from_mode(DIR_MODE);
    fs::set_permissions(path, perms)?;
    Ok(())
}

pub fn write_private_file(path: &Path, contents: impl AsRef<[u8]>) -> Result<()> {
    if let Some(parent) = path.parent() {
        ensure_private_dir(parent)?;
    }

    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(FILE_MODE)
        .open(path)
        .map_err(|e| VoxError::Io(e))?;

    file.write_all(contents.as_ref())?;
    file.sync_all()?;

    let perms = fs::Permissions::from_mode(FILE_MODE);
    fs::set_permissions(path, perms)?;
    Ok(())
}
