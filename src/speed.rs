//! Transcription speed relative to the audio, and whether a dictation was
//! unusually slow.
//!
//! The 0.3.0 Windows build once ran Whisper ~6x slower than it should
//! (MSVC codegen), and every dictation took 20-70 s with nothing telling
//! the user why. Speed is now measured per dictation so the tray can say
//! so, and `vox doctor` can flag a machine or build that's slow overall.

use crate::history::DictationRecord;
use crate::model::LatencyMetrics;

/// Shorter clips are dominated by fixed overhead (and padded to 1.25 s
/// before Whisper sees them), so their ratio says little about speed.
pub const MIN_AUDIO_MS: u64 = 1500;
/// Slower than real time: speaking for 4 s takes more than 4 s to
/// transcribe. The base model runs at 0.1-0.5x on any recent CPU.
pub const SLOW_FACTOR: f64 = 1.0;
/// With a baseline, only a dictation this many times slower than usual
/// counts as a slowdown; a machine that's always this speed is a `doctor`
/// finding, not something to repeat in the tray after every dictation.
pub const BASELINE_MULTIPLE: f64 = 2.0;
const BASELINE_MIN_SAMPLES: usize = 5;
const BASELINE_WINDOW: usize = 20;

/// Seconds of transcription per second of speech (lower is faster). For a
/// streamed dictation this is Whisper's total time across its segments, not
/// the short wait after stop, so a slow build still shows up as slow.
pub fn realtime_factor(latency: &LatencyMetrics) -> Option<f64> {
    (latency.audio_ms >= MIN_AUDIO_MS)
        .then(|| latency.stt_compute_ms() as f64 / latency.audio_ms as f64)
}

/// Realtime factors of the most recent dictations that have one, newest
/// first, at most `limit`.
pub fn recent_factors(records: &[DictationRecord], limit: usize) -> Vec<f64> {
    let mut sorted: Vec<&DictationRecord> = records.iter().collect();
    sorted.sort_by_key(|r| std::cmp::Reverse(r.created_at_ms));
    sorted
        .into_iter()
        .filter_map(|r| realtime_factor(&r.latency))
        .take(limit)
        .collect()
}

pub fn median(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.total_cmp(b));
    let mid = v.len() / 2;
    Some(if v.len().is_multiple_of(2) {
        (v[mid - 1] + v[mid]) / 2.0
    } else {
        v[mid]
    })
}

/// The usual speed on this machine: median of recent dictations, once
/// there are enough of them to mean something.
pub fn baseline(records: &[DictationRecord]) -> Option<f64> {
    let factors = recent_factors(records, BASELINE_WINDOW);
    if factors.len() < BASELINE_MIN_SAMPLES {
        return None;
    }
    median(&factors)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Slowdown {
    pub factor: f64,
    pub transcribe_ms: u64,
    pub audio_ms: u64,
}

/// `Some` when `latest` was slower than real time and, if this machine has
/// a baseline, also well beyond it. `earlier` must not include `latest`.
pub fn check(latest: &LatencyMetrics, earlier: &[DictationRecord]) -> Option<Slowdown> {
    let factor = realtime_factor(latest)?;
    if factor < SLOW_FACTOR {
        return None;
    }
    if let Some(usual) = baseline(earlier) {
        if factor < usual * BASELINE_MULTIPLE {
            return None;
        }
    }
    Some(Slowdown {
        factor,
        transcribe_ms: latest.stt_compute_ms(),
        audio_ms: latest.audio_ms,
    })
}

/// Tray status for a slow dictation, with the likely cause when one can
/// be detected.
pub fn status_text(slow: &Slowdown, on_battery: Option<bool>) -> String {
    let base = format!(
        "Vox: slow - {:.1}s for {:.1}s of speech",
        slow.transcribe_ms as f64 / 1000.0,
        slow.audio_ms as f64 / 1000.0
    );
    match on_battery {
        Some(true) => format!("{base} (on battery: plug in for full speed)"),
        _ => format!("{base} (CPU busy? see vox doctor)"),
    }
}

/// Human wording for a factor: "3.2x faster than real time".
pub fn describe(factor: f64) -> String {
    if factor <= 0.0 {
        "instant".into()
    } else if factor < 1.0 {
        format!("{:.1}x faster than real time", 1.0 / factor)
    } else {
        format!("{factor:.1}x slower than real time")
    }
}

/// `Some(true)` when running on battery, `None` when it can't be told
/// (desktops, VMs, unsupported platforms).
pub fn on_battery() -> Option<bool> {
    platform::on_battery()
}

#[cfg(windows)]
mod platform {
    use windows_sys::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};

    pub fn on_battery() -> Option<bool> {
        // SAFETY: GetSystemPowerStatus only writes the struct we pass it.
        let mut status: SYSTEM_POWER_STATUS = unsafe { std::mem::zeroed() };
        if unsafe { GetSystemPowerStatus(&mut status) } == 0 {
            return None;
        }
        match status.ACLineStatus {
            0 => Some(true),
            1 => Some(false),
            _ => None,
        }
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use std::fs;

    /// A laptop has a "Mains" supply whose `online` is 0 on battery.
    pub fn on_battery() -> Option<bool> {
        let mut saw_mains = false;
        for entry in fs::read_dir("/sys/class/power_supply").ok()?.flatten() {
            let path = entry.path();
            let kind = fs::read_to_string(path.join("type")).unwrap_or_default();
            if kind.trim() != "Mains" {
                continue;
            }
            saw_mains = true;
            if fs::read_to_string(path.join("online"))
                .unwrap_or_default()
                .trim()
                == "1"
            {
                return Some(false);
            }
        }
        saw_mains.then_some(true)
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::process::Command;

    pub fn on_battery() -> Option<bool> {
        let out = Command::new("pmset").args(["-g", "batt"]).output().ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        if text.contains("'Battery Power'") {
            Some(true)
        } else if text.contains("'AC Power'") {
            Some(false)
        } else {
            None
        }
    }
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
mod platform {
    pub fn on_battery() -> Option<bool> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Format, Mode, Privacy};

    fn latency(transcribe_ms: u64, audio_ms: u64) -> LatencyMetrics {
        LatencyMetrics {
            transcribe_ms,
            audio_ms,
            ..LatencyMetrics::default()
        }
    }

    fn record(at: u64, transcribe_ms: u64, audio_ms: u64) -> DictationRecord {
        DictationRecord {
            id: format!("dict-{at}"),
            created_at_ms: at,
            app_name: "test".into(),
            bundle_id: "test".into(),
            window_title: None,
            format: Format::CleanProse,
            privacy: Privacy::LocalOnly,
            mode: Mode::Auto,
            raw_text: String::new(),
            model_text: None,
            final_text: String::new(),
            injection_strategy: String::new(),
            injected_chars: 0,
            latency: latency(transcribe_ms, audio_ms),
            corrected_text: None,
            stt_confidence: None,
            rejected: None,
            edit: false,
        }
    }

    #[test]
    fn factor_needs_enough_audio() {
        assert_eq!(realtime_factor(&latency(500, 1000)), None);
        assert_eq!(realtime_factor(&latency(1000, 4000)), Some(0.25));
        // Records from before audio_ms existed deserialize as 0.
        assert_eq!(realtime_factor(&latency(1000, 0)), None);
    }

    #[test]
    fn streamed_dictations_are_judged_on_whisper_time_not_the_wait() {
        // 40 s of speech, 1.1 s wait after stop, but Whisper ran 50 s in all.
        let streamed = LatencyMetrics {
            transcribe_ms: 1_100,
            audio_ms: 40_000,
            streamed_segments: 7,
            stt_work_ms: 50_000,
            ..LatencyMetrics::default()
        };
        assert_eq!(realtime_factor(&streamed), Some(1.25));
        let slow = check(&streamed, &[]).expect("slower than real time");
        assert_eq!(slow.transcribe_ms, 50_000);
    }

    #[test]
    fn slow_without_history_means_slower_than_real_time() {
        assert!(check(&latency(2000, 4000), &[]).is_none());
        let slow = check(&latency(20_000, 4000), &[]).unwrap();
        assert_eq!(slow.factor, 5.0);
    }

    #[test]
    fn baseline_suppresses_a_consistently_slow_machine() {
        // Always ~1.2x real time: the tray shouldn't nag on every dictation.
        let usual: Vec<_> = (0..6).map(|i| record(i, 4800, 4000)).collect();
        assert!(check(&latency(5200, 4000), &usual).is_none());
        // ...but a dictation far beyond the usual still counts.
        assert!(check(&latency(12_000, 4000), &usual).is_some());
    }

    #[test]
    fn baseline_from_fast_history_flags_a_regression() {
        // Usually 0.3x; today 7x (the MSVC build) - flagged.
        let usual: Vec<_> = (0..10).map(|i| record(i, 1200, 4000)).collect();
        assert!((baseline(&usual).unwrap() - 0.3).abs() < 1e-9);
        assert!(check(&latency(28_000, 4000), &usual).is_some());
    }

    #[test]
    fn baseline_ignores_short_clips_and_uses_recent_ones() {
        let mut recs: Vec<_> = (0..4).map(|i| record(i, 900, 900)).collect();
        assert_eq!(baseline(&recs), None);
        // 30 old fast ones, then 20 recent slow ones: the recent ones win.
        recs = (0..30).map(|i| record(i, 400, 4000)).collect();
        recs.extend((30..50).map(|i| record(i, 2000, 4000)));
        assert_eq!(baseline(&recs), Some(0.5));
    }

    #[test]
    fn describes_speed() {
        assert_eq!(describe(0.25), "4.0x faster than real time");
        assert_eq!(describe(3.6), "3.6x slower than real time");
        assert_eq!(median(&[3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(&[4.0, 1.0, 2.0, 3.0]), Some(2.5));
    }

    #[test]
    fn status_names_the_battery() {
        let slow = Slowdown {
            factor: 5.0,
            transcribe_ms: 20_000,
            audio_ms: 4000,
        };
        assert!(status_text(&slow, Some(true)).contains("on battery"));
        assert!(status_text(&slow, None).contains("vox doctor"));
    }
}
