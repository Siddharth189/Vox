use std::fs;
use std::ops::Range;
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
    /// Words to add to the dictionary first, usually ones a previous
    /// response suggested.
    #[serde(default)]
    pub add_terms: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorrectionResult {
    pub learned_aliases: Vec<LearnedAlias>,
    /// Words the correction introduced that look like names or jargon and
    /// aren't in the dictionary yet ("Vox" from "voxand" -> "Vox and").
    #[serde(default)]
    pub suggested_terms: Vec<String>,
    #[serde(default)]
    pub added_terms: Vec<String>,
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
    let json = serde_json::to_string_pretty(&sorted).map_err(|e| VoxError::Other(e.to_string()))?;
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

/// A run of words the user changed: `original` became `corrected`. Either
/// side may be empty (a pure insertion or deletion).
#[derive(Debug, Clone, PartialEq)]
struct Hunk {
    original: Range<usize>,
    corrected: Range<usize>,
}

/// Beyond this the word diff's O(n*m) table isn't worth it; a dictation
/// is rarely more than a few hundred words.
const MAX_DIFF_CELLS: usize = 1_000_000;

/// Word-level diff (case-insensitive LCS). Lining words up by position
/// broke as soon as one word was split or merged ("voxand" -> "Vox and"):
/// every word after it was compared with the wrong one.
fn diff_hunks(original: &[String], corrected: &[String]) -> Vec<Hunk> {
    let (n, m) = (original.len(), corrected.len());
    if n.saturating_mul(m) > MAX_DIFF_CELLS {
        return Vec::new();
    }
    let eq = |i: usize, j: usize| original[i].eq_ignore_ascii_case(&corrected[j]);
    // lcs[i][j] = LCS length of original[i..] and corrected[j..].
    let mut lcs = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if eq(i, j) {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let mut hunks = Vec::new();
    let (mut i, mut j) = (0, 0);
    let (mut from_i, mut from_j) = (0, 0);
    while i < n || j < m {
        if i < n && j < m && eq(i, j) {
            if from_i < i || from_j < j {
                hunks.push(Hunk {
                    original: from_i..i,
                    corrected: from_j..j,
                });
            }
            i += 1;
            j += 1;
            (from_i, from_j) = (i, j);
        } else if j < m && (i == n || lcs[i][j + 1] >= lcs[i + 1][j]) {
            j += 1;
        } else {
            i += 1;
        }
    }
    if from_i < n || from_j < m {
        hunks.push(Hunk {
            original: from_i..n,
            corrected: from_j..m,
        });
    }
    hunks
}

/// Where corrected word boundary `pos` falls in the original. Unchanged
/// stretches map one-to-one; a boundary inside a hunk snaps to that hunk's
/// start (`at_end` false) or end (true).
fn to_original(hunks: &[Hunk], pos: usize, at_end: bool) -> usize {
    let mut shift: isize = 0;
    for h in hunks {
        if pos <= h.corrected.start {
            break;
        }
        if pos < h.corrected.end {
            return if at_end {
                h.original.end
            } else {
                h.original.start
            };
        }
        shift = h.original.end as isize - h.corrected.end as isize;
    }
    (pos as isize + shift) as usize
}

const MAX_SUGGESTIONS: usize = 5;

/// Everyday words a correction often fixes ("there" -> "their") that
/// would only clutter the dictionary.
const COMMON_WORDS: &[&str] = &[
    "a", "about", "after", "again", "all", "also", "an", "and", "any", "are", "as", "at", "be",
    "because", "been", "before", "but", "by", "can", "could", "day", "did", "do", "does", "done",
    "for", "from", "get", "go", "good", "got", "had", "has", "have", "he", "her", "here", "him",
    "his", "how", "i", "if", "in", "into", "is", "it", "its", "just", "know", "like", "make", "me",
    "more", "my", "need", "new", "no", "not", "now", "of", "on", "one", "only", "or", "other",
    "our", "out", "over", "please", "said", "see", "she", "should", "so", "some", "than", "thanks",
    "that", "the", "their", "them", "then", "there", "these", "they", "think", "this", "those",
    "time", "to", "too", "two", "up", "us", "very", "want", "was", "way", "we", "well", "were",
    "what", "when", "where", "which", "who", "why", "will", "with", "work", "would", "yes", "you",
    "your",
];

/// Whether a word the user typed looks like something Whisper won't know:
/// a name, acronym, or product ("Vox", "GPT-4"), or a lowercase word that
/// replaced misheard speech on its own ("cube cuddle" -> "kubectl").
fn looks_like_term(word: &str, replaced_alone: bool) -> bool {
    if word.len() < 2 || word.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    if word
        .chars()
        .any(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
    {
        return true;
    }
    replaced_alone && !COMMON_WORDS.contains(&word.to_ascii_lowercase().as_str())
}

pub fn learn_from_correction(
    settings: &mut Settings,
    req: &CorrectionRequest,
) -> Result<CorrectionResult> {
    let mut added = Vec::new();
    for term in &req.add_terms {
        let term = term.trim();
        let known = settings
            .custom_dictionary
            .iter()
            .any(|t| t.trim().eq_ignore_ascii_case(term));
        if !term.is_empty() && !known {
            settings.custom_dictionary.push(term.to_string());
            added.push(term.to_string());
        }
    }

    let original = tokenize(&req.original_text);
    let corrected = tokenize(&req.corrected_text);
    let hunks: Vec<Hunk> = diff_hunks(&original, &corrected)
        .into_iter()
        .filter(|h| !h.original.is_empty() && !h.corrected.is_empty())
        .collect();
    let mut explained = vec![false; hunks.len()];

    // Each dictionary term the corrected text contains, where the user
    // changed words inside it: what Whisper heard there is an alias
    // ("coopernetes" -> "Kubernetes", "visual studio coat" -> "Visual
    // Studio Code"). A change that spills past the term ("voxand" ->
    // "Vox and") isn't: the alias would swallow the extra word.
    let mut learned = Vec::new();
    for term in settings.custom_dictionary.clone() {
        let term_words = tokenize(&term);
        let len = term_words.len();
        if len == 0 || len > corrected.len() {
            continue;
        }
        for start in 0..=corrected.len() - len {
            let window = &corrected[start..start + len];
            if !window
                .iter()
                .zip(&term_words)
                .all(|(a, b)| a.eq_ignore_ascii_case(b))
            {
                continue;
            }
            let span = start..start + len;
            let touching: Vec<usize> = (0..hunks.len())
                .filter(|&k| {
                    hunks[k].corrected.start < span.end && span.start < hunks[k].corrected.end
                })
                .collect();
            let contained = touching.iter().all(|&k| {
                span.start <= hunks[k].corrected.start && hunks[k].corrected.end <= span.end
            });
            if touching.is_empty() || !contained {
                continue;
            }
            let heard = original
                [to_original(&hunks, span.start, false)..to_original(&hunks, span.end, true)]
                .join(" ");
            for &k in &touching {
                explained[k] = true;
            }
            let entry = settings
                .custom_aliases
                .entry(term.trim().to_string())
                .or_default();
            if !heard.is_empty()
                && !heard.eq_ignore_ascii_case(term.trim())
                && !entry.iter().any(|a| a.eq_ignore_ascii_case(&heard))
            {
                entry.push(heard.clone());
                learned.push(LearnedAlias {
                    term: term.trim().to_string(),
                    alias: heard,
                });
            }
        }
    }

    let mut suggested: Vec<String> = Vec::new();
    for (k, hunk) in hunks.iter().enumerate() {
        if explained[k] {
            continue;
        }
        let words = &corrected[hunk.corrected.clone()];
        for word in words {
            let known = settings
                .custom_dictionary
                .iter()
                .any(|t| t.trim().eq_ignore_ascii_case(word))
                || suggested.iter().any(|s| s.eq_ignore_ascii_case(word));
            if suggested.len() < MAX_SUGGESTIONS
                && !known
                && looks_like_term(word, words.len() == 1)
            {
                suggested.push(word.clone());
            }
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
        suggested_terms: suggested,
        added_terms: added,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::{Mutex, MutexGuard, OnceLock};

    /// Points VOX_DATA_DIR at one temp dir for every test here, so saves
    /// stay out of the real data dir on every OS (HOME isn't consulted on
    /// Windows), and makes the tests take turns: they run in parallel and
    /// all write the same settings.yaml.
    fn test_env() -> MutexGuard<'static, ()> {
        static DIR: OnceLock<tempfile::TempDir> = OnceLock::new();
        static LOCK: Mutex<()> = Mutex::new(());
        let guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let dir = DIR.get_or_init(|| tempfile::tempdir().unwrap());
        // SAFETY: test-only, and only ever set to this one value while
        // holding LOCK.
        unsafe { std::env::set_var("VOX_DATA_DIR", dir.path()) };
        guard
    }

    #[test]
    fn learns_position_aligned_alias() {
        let _env = test_env();
        let mut settings = Settings::with_defaults();
        settings.custom_dictionary = vec!["Kubernetes".into()];
        let req = CorrectionRequest {
            record_id: None,
            original_text: "deploy to coopernetes staging".into(),
            corrected_text: "deploy to Kubernetes staging".into(),
            add_terms: Vec::new(),
        };
        let result = learn_from_correction(&mut settings, &req).unwrap();
        assert_eq!(result.learned_aliases.len(), 1);
        assert_eq!(result.learned_aliases[0].term, "Kubernetes");
        assert_eq!(result.learned_aliases[0].alias, "coopernetes");
    }

    fn hunk(o: Range<usize>, c: Range<usize>) -> Hunk {
        Hunk {
            original: o,
            corrected: c,
        }
    }

    #[test]
    fn diff_finds_split_words_without_misaligning_the_rest() {
        let hunks = diff_hunks(
            &tokenize("this text is written using voxand it works"),
            &tokenize("this text is written using Vox and it works"),
        );
        assert_eq!(hunks, vec![hunk(5..6, 5..7)]);
        assert!(diff_hunks(&tokenize("same words"), &tokenize("Same Words")).is_empty());
        let ins = diff_hunks(&tokenize("ship it"), &tokenize("ship it today"));
        assert_eq!(ins, vec![hunk(2..2, 2..3)]);
    }

    #[test]
    fn maps_corrected_positions_back_to_the_original() {
        // "run cube cuddle apply now" -> "run kubectl apply now"
        let hunks = vec![hunk(1..3, 1..2)];
        assert_eq!(to_original(&hunks, 1, false), 1);
        assert_eq!(to_original(&hunks, 2, true), 3);
        assert_eq!(to_original(&hunks, 3, true), 4);
        assert_eq!(to_original(&hunks, 0, false), 0);
    }

    fn isolated_settings() -> (MutexGuard<'static, ()>, Settings) {
        (test_env(), Settings::with_defaults())
    }

    fn correction(original: &str, corrected: &str, add: &[&str]) -> CorrectionRequest {
        CorrectionRequest {
            record_id: None,
            original_text: original.into(),
            corrected_text: corrected.into(),
            add_terms: add.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn suggests_new_names_but_not_ordinary_words() {
        let (_dir, mut settings) = isolated_settings();
        let req = correction(
            "written using voxand it works",
            "written using Vox and it works",
            &[],
        );
        let result = learn_from_correction(&mut settings, &req).unwrap();
        assert_eq!(result.suggested_terms, vec!["Vox".to_string()]);
        assert!(result.learned_aliases.is_empty());
        // Suggestions aren't added until the user accepts them.
        assert!(settings.custom_dictionary.is_empty());

        let req = correction("put it over there", "put it over their", &[]);
        let result = learn_from_correction(&mut settings, &req).unwrap();
        assert!(result.suggested_terms.is_empty());
    }

    #[test]
    fn accepting_a_split_word_suggestion_adds_it_without_a_greedy_alias() {
        let (_dir, mut settings) = isolated_settings();
        let req = correction("using voxand it works", "using Vox and it works", &["Vox"]);
        let result = learn_from_correction(&mut settings, &req).unwrap();
        assert_eq!(result.added_terms, vec!["Vox".to_string()]);
        // "voxand" -> "Vox" would delete the "and"; Whisper's prompt
        // handles this case instead.
        assert!(result.learned_aliases.is_empty());
        assert!(settings.custom_dictionary.iter().any(|t| t == "Vox"));
    }

    #[test]
    fn learns_what_whisper_heard_for_lowercase_jargon() {
        let (_dir, mut settings) = isolated_settings();
        let req = correction("run cube cuddle apply now", "run kubectl apply now", &[]);
        let first = learn_from_correction(&mut settings, &req).unwrap();
        assert_eq!(first.suggested_terms, vec!["kubectl".to_string()]);

        let req = correction(
            "run cube cuddle apply now",
            "run kubectl apply now",
            &["kubectl"],
        );
        let result = learn_from_correction(&mut settings, &req).unwrap();
        assert_eq!(result.learned_aliases.len(), 1);
        assert_eq!(result.learned_aliases[0].alias, "cube cuddle");
        assert!(result.suggested_terms.is_empty());
    }

    #[test]
    fn learns_multi_word_terms() {
        let (_dir, mut settings) = isolated_settings();
        settings.custom_dictionary = vec!["Visual Studio Code".into()];
        let req = correction(
            "open visual studio coat now",
            "open Visual Studio Code now",
            &[],
        );
        let result = learn_from_correction(&mut settings, &req).unwrap();
        assert_eq!(result.learned_aliases.len(), 1);
        assert_eq!(result.learned_aliases[0].term, "Visual Studio Code");
        assert_eq!(result.learned_aliases[0].alias, "visual studio coat");
        assert!(result.suggested_terms.is_empty());
    }
}
