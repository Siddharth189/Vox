//! Lifetime usage totals: how much you've dictated and the typing time it
//! saved. Kept apart from history.json, which only holds the last 100
//! dictations, and stores counts only - never any text.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config;
use crate::error::{Result, VoxError};
use crate::secure_fs;

/// Typing speed the "time saved" figure is measured against: a typical
/// typist, not a fast one.
pub const TYPING_WPM: f64 = 40.0;

const MS_PER_DAY: u64 = 86_400_000;
/// Daily word counts kept for streaks and the 7-day figure.
const DAYS_KEPT: usize = 400;

fn stats_path() -> PathBuf {
    config::data_dir().join("stats.json")
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageStats {
    #[serde(default)]
    pub dictations: u64,
    #[serde(default)]
    pub words: u64,
    #[serde(default)]
    pub speech_ms: u64,
    #[serde(default)]
    pub first_at_ms: u64,
    /// Words per day, keyed by days since the Unix epoch (UTC).
    #[serde(default)]
    pub daily_words: BTreeMap<u64, u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StatsSummary {
    pub dictations: u64,
    pub words: u64,
    pub speaking_minutes: f64,
    pub time_saved_minutes: f64,
    /// Average speaking pace over all dictations.
    pub words_per_minute: f64,
    pub words_today: u64,
    pub words_last_7_days: u64,
    /// Consecutive days with at least one dictation, ending today (or
    /// yesterday, so the streak isn't shown as broken before you've
    /// dictated today).
    pub streak_days: u64,
    pub typing_wpm_baseline: f64,
    pub first_at_ms: u64,
}

impl UsageStats {
    pub fn record(&mut self, words: u64, audio_ms: u64, now_ms: u64) {
        if words == 0 {
            return;
        }
        self.dictations += 1;
        self.words += words;
        self.speech_ms += audio_ms;
        if self.first_at_ms == 0 {
            self.first_at_ms = now_ms;
        }
        *self.daily_words.entry(now_ms / MS_PER_DAY).or_default() += words;
        while self.daily_words.len() > DAYS_KEPT {
            self.daily_words.pop_first();
        }
    }

    pub fn summary(&self, now_ms: u64) -> StatsSummary {
        let today = now_ms / MS_PER_DAY;
        let speaking_minutes = self.speech_ms as f64 / 60_000.0;
        let typing_minutes = self.words as f64 / TYPING_WPM;
        let words_on = |day: u64| self.daily_words.get(&day).copied().unwrap_or(0);

        let mut day = if words_on(today) > 0 {
            today
        } else {
            today.saturating_sub(1)
        };
        let mut streak_days = 0;
        while words_on(day) > 0 {
            streak_days += 1;
            if day == 0 {
                break;
            }
            day -= 1;
        }

        StatsSummary {
            dictations: self.dictations,
            words: self.words,
            speaking_minutes: round1(speaking_minutes),
            time_saved_minutes: round1((typing_minutes - speaking_minutes).max(0.0)),
            words_per_minute: if speaking_minutes > 0.0 {
                (self.words as f64 / speaking_minutes).round()
            } else {
                0.0
            },
            words_today: words_on(today),
            words_last_7_days: (today.saturating_sub(6)..=today).map(words_on).sum(),
            streak_days,
            typing_wpm_baseline: TYPING_WPM,
            first_at_ms: self.first_at_ms,
        }
    }
}

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

pub fn load() -> UsageStats {
    fs::read_to_string(stats_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save(stats: &UsageStats) -> Result<()> {
    let json = serde_json::to_string(stats).map_err(|e| VoxError::Other(e.to_string()))?;
    secure_fs::write_private_file(&stats_path(), json)
}

/// Adds one finished dictation to the totals on disk.
pub fn record_dictation(text: &str, audio_ms: u64) -> Result<()> {
    let words = text.split_whitespace().count() as u64;
    if words == 0 {
        return Ok(());
    }
    let mut stats = load();
    stats.record(words, audio_ms, crate::history::now_ms());
    save(&stats)
}

pub fn reset() -> Result<()> {
    save(&UsageStats::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: u64 = MS_PER_DAY;

    #[test]
    fn totals_and_time_saved() {
        let mut s = UsageStats::default();
        // 120 words in 1 minute of speech: 3 minutes to type at 40 wpm
        s.record(80, 40_000, 100 * DAY);
        s.record(40, 20_000, 100 * DAY + 5);
        s.record(0, 9_000, 100 * DAY + 9); // silence doesn't count
        let sum = s.summary(100 * DAY + 10);
        assert_eq!(sum.dictations, 2);
        assert_eq!(sum.words, 120);
        assert_eq!(sum.speaking_minutes, 1.0);
        assert_eq!(sum.time_saved_minutes, 2.0);
        assert_eq!(sum.words_per_minute, 120.0);
        assert_eq!(sum.words_today, 120);
        assert_eq!(sum.first_at_ms, 100 * DAY);
    }

    #[test]
    fn streak_counts_back_from_today_or_yesterday() {
        let mut s = UsageStats::default();
        for day in [90, 97, 98, 99] {
            s.record(10, 1_000, day * DAY);
        }
        // nothing yet on day 100: yesterday's streak still stands
        assert_eq!(s.summary(100 * DAY).streak_days, 3);
        s.record(5, 1_000, 100 * DAY);
        let sum = s.summary(100 * DAY);
        assert_eq!(sum.streak_days, 4);
        assert_eq!(sum.words_last_7_days, 35);
        // a missed day breaks it
        assert_eq!(s.summary(102 * DAY).streak_days, 0);
    }

    #[test]
    fn old_days_are_pruned() {
        let mut s = UsageStats::default();
        for day in 0..(DAYS_KEPT as u64 + 10) {
            s.record(1, 100, day * DAY);
        }
        assert_eq!(s.daily_words.len(), DAYS_KEPT);
        assert_eq!(s.words, DAYS_KEPT as u64 + 10);
    }
}
