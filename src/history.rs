use std::fs;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::config::{self, Settings};
use crate::error::{Result, VoxError};
use crate::model::{Format, LatencyMetrics, Mode, Privacy};
use crate::secure_fs;

pub const HISTORY_LIMIT: usize = 100;

static SEQ: AtomicU32 = AtomicU32::new(0);

fn history_path() -> std::path::PathBuf {
    config::data_dir().join("history.json")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DictationRecord {
    pub id: String,
    pub created_at_ms: u64,
    pub app_name: String,
    pub bundle_id: String,
    pub window_title: Option<String>,
    pub format: Format,
    pub privacy: Privacy,
    pub mode: Mode,
    pub raw_text: String,
    pub model_text: Option<String>,
    pub final_text: String,
    pub injection_strategy: String,
    pub injected_chars: usize,
    #[serde(default)]
    pub latency: LatencyMetrics,
    pub corrected_text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearnedAlias {
    pub term: String,
    pub alias: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorrectionRequest {
    pub record_id: Option<String>,
    pub original_text: String,
    pub corrected_text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorrectionResult {
    pub learned_aliases: Vec<LearnedAlias>,
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn new_record_id(created_at_ms: u64) -> String {
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("dict-{created_at_ms}-{seq}")
}

pub fn load_history() -> Vec<DictationRecord> {
    let path = history_path();
    match fs::read_to_string(&path) {
        Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

pub fn save_history(records: &[DictationRecord]) -> Result<()> {
    let path = history_path();
    let mut sorted = records.to_vec();
    sorted.sort_by_key(|r| r.created_at_ms);
    if sorted.len() > HISTORY_LIMIT {
        let skip = sorted.len() - HISTORY_LIMIT;
        sorted = sorted.split_off(skip);
    }
    let json = serde_json::to_string_pretty(&sorted)
        .map_err(|e| VoxError::Other(e.to_string()))?;
    secure_fs::write_private_file(&path, json)?;
    Ok(())
}

pub fn append_record(record: DictationRecord) -> Result<()> {
    let mut records = load_history();
    records.push(record);
    save_history(&records)
}

pub fn clear_history() -> Result<()> {
    save_history(&[])
}

pub fn latest_record() -> Option<DictationRecord> {
    let mut records = load_history();
    records.sort_by_key(|r| std::cmp::Reverse(r.created_at_ms));
    records.into_iter().next()
}

fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '/' || c == '-'))
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string())
        .collect()
}

pub fn learn_from_correction(
    settings: &mut Settings,
    req: &CorrectionRequest,
) -> Result<CorrectionResult> {
    let original_tokens = tokenize(&req.original_text);
    let corrected_tokens = tokenize(&req.corrected_text);

    let mut learned = Vec::new();
    let dictionary = settings.custom_dictionary.clone();

    for term in &dictionary {
        let term_trim = term.trim();
        if term_trim.is_empty() {
            continue;
        }
        let in_corrected = corrected_tokens
            .iter()
            .any(|t| t.eq_ignore_ascii_case(term_trim));
        let in_original = original_tokens
            .iter()
            .any(|t| t.eq_ignore_ascii_case(term_trim));
        if !in_corrected || in_original {
            continue;
        }

        let Some(idx) = corrected_tokens
            .iter()
            .position(|t| t.eq_ignore_ascii_case(term_trim))
        else {
            continue;
        };
        let Some(alias) = original_tokens.get(idx) else {
            continue;
        };
        if alias.eq_ignore_ascii_case(term_trim) {
            continue;
        }

        let entry = settings
            .custom_aliases
            .entry(term_trim.to_string())
            .or_default();
        let already = entry.iter().any(|a| a.eq_ignore_ascii_case(alias));
        if !already {
            entry.push(alias.clone());
            learned.push(LearnedAlias {
                term: term_trim.to_string(),
                alias: alias.clone(),
            });
        }
    }

    settings.sanitize();
    settings.save()?;

    if let Some(ref id) = req.record_id {
        let mut records = load_history();
        if let Some(rec) = records.iter_mut().find(|r| &r.id == id) {
            rec.corrected_text = Some(req.corrected_text.clone());
        }
        save_history(&records)?;
    }

    Ok(CorrectionResult {
        learned_aliases: learned,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn learns_position_aligned_alias() {
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: test-only, single-threaded test process
        unsafe { std::env::set_var("HOME", dir.path()) };

        let mut settings = Settings::with_defaults();
        settings.custom_dictionary = vec!["Kubernetes".into()];
        let req = CorrectionRequest {
            record_id: None,
            original_text: "deploy to coopernetes staging".into(),
            corrected_text: "deploy to Kubernetes staging".into(),
        };
        let result = learn_from_correction(&mut settings, &req).unwrap();
        assert_eq!(result.learned_aliases.len(), 1);
        assert_eq!(result.learned_aliases[0].term, "Kubernetes");
        assert_eq!(result.learned_aliases[0].alias, "coopernetes");
    }
}
