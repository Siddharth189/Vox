//! Whisper model download, shared by `vox download-model` and the tray's
//! first-run setup (no bash/curl needed, same behaviour on every OS).

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::config;
use crate::error::{Result, VoxError};

const BASE_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main";
const GGML_MAGIC: [u8; 4] = [0x6c, 0x6d, 0x67, 0x67]; // "lmgg"

/// Known-good choices, smallest first, with approximate download sizes.
pub const KNOWN_MODELS: &[(&str, &str)] = &[
    (
        "tiny.en-q5_1",
        "31 MB, English only, fastest, least accurate",
    ),
    ("base.en-q5_1", "57 MB, English only, fast"),
    ("base-q5_1", "57 MB, multilingual, fast (default)"),
    ("small.en-q5_1", "181 MB, English only, more accurate"),
    ("small-q5_1", "181 MB, multilingual, more accurate"),
    (
        "large-v3-turbo-q5_0",
        "547 MB, multilingual, most accurate, slow on CPU",
    ),
];

/// Accepts "base", "ggml-base.bin", "base-q5_1", ... and rejects anything
/// that could escape the models directory.
pub fn normalize_name(name: &str) -> Result<String> {
    let n = name.trim();
    let n = n.strip_prefix("ggml-").unwrap_or(n);
    let n = n.strip_suffix(".bin").unwrap_or(n);
    if n.is_empty()
        || n.len() > 64
        || !n
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        || n.contains("..")
    {
        return Err(VoxError::Config(format!("invalid model name `{name}`")));
    }
    Ok(n.to_string())
}

pub fn is_valid_model_file(path: &Path) -> bool {
    let mut magic = [0u8; 4];
    File::open(path)
        .and_then(|mut f| f.read_exact(&mut magic))
        .map(|_| magic == GGML_MAGIC)
        .unwrap_or(false)
}

/// Downloads `ggml-<name>.bin` into the models directory (no-op if a valid
/// copy is already there) and returns its path. `progress(done, total)` is
/// called as bytes arrive.
pub fn download(name: &str, mut progress: impl FnMut(u64, Option<u64>)) -> Result<PathBuf> {
    let name = normalize_name(name)?;
    let dest = config::whisper_model_path(&name);
    if dest.exists() && is_valid_model_file(&dest) {
        return Ok(dest);
    }
    let dir = config::model_dir();
    crate::secure_fs::ensure_private_dir(&dir)?;

    let url = format!("{BASE_URL}/ggml-{name}.bin");
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(None)
        .build()
        .map_err(|e| VoxError::Other(e.to_string()))?;
    let mut resp = client
        .get(&url)
        .send()
        .map_err(|e| VoxError::Other(format!("download failed: {e}")))?;
    if !resp.status().is_success() {
        return Err(VoxError::Other(format!(
            "download failed: {url} returned HTTP {}",
            resp.status()
        )));
    }
    let total = resp.content_length();

    let tmp = dir.join(format!(".ggml-{name}.{}.part", std::process::id()));
    let result = (|| -> Result<()> {
        let mut file = File::create(&tmp)?;
        let mut buf = vec![0u8; 256 * 1024];
        let mut done = 0u64;
        loop {
            let n = resp
                .read(&mut buf)
                .map_err(|e| VoxError::Other(format!("download interrupted: {e}")))?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n])?;
            done += n as u64;
            progress(done, total);
        }
        file.sync_all()?;
        drop(file);
        if let Some(t) = total {
            if done != t {
                return Err(VoxError::Other(format!(
                    "download incomplete: got {done} of {t} bytes"
                )));
            }
        }
        if !is_valid_model_file(&tmp) {
            return Err(VoxError::Other(
                "downloaded file is not a ggml whisper model (bad magic bytes)".into(),
            ));
        }
        fs::rename(&tmp, &dest)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.map(|_| dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_normalized_and_sandboxed() {
        assert_eq!(normalize_name("base").unwrap(), "base");
        assert_eq!(
            normalize_name("ggml-base.en-q5_1.bin").unwrap(),
            "base.en-q5_1"
        );
        assert!(normalize_name("../../etc/passwd").is_err());
        assert!(normalize_name("a/b").is_err());
        assert!(normalize_name(r"a\b").is_err());
        assert!(normalize_name("").is_err());
    }

    #[test]
    fn magic_check() {
        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join("good.bin");
        let bad = dir.path().join("bad.bin");
        fs::write(&good, [0x6c, 0x6d, 0x67, 0x67, 1, 2, 3]).unwrap();
        fs::write(&bad, b"<html>404</html>").unwrap();
        assert!(is_valid_model_file(&good));
        assert!(!is_valid_model_file(&bad));
        assert!(!is_valid_model_file(&dir.path().join("missing.bin")));
    }
}
