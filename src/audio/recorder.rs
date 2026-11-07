use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};

use crate::error::{Result, VoxError};
use crate::model::AudioChunk;

pub const TARGET_RATE: u32 = 16_000;

/// Hard cap on a single dictation. Protects against a lost key-up (screen
/// lock, focus change mid-hold) recording forever; the tray auto-stops and
/// transcribes when it's reached.
pub const MAX_RECORDING: Duration = Duration::from_secs(300);

const WINDOW_MS: u32 = 20;
const PADDING_MS: u32 = 100;
const THRESHOLD_RATIO: f32 = 0.1;
const PEAK_PERCENTILE: f32 = 0.95;

/// Loudest 20 ms window below this RMS means nobody spoke. Whisper
/// hallucinates stock phrases ("Thank you.", "you") on pure room noise, so
/// such clips never reach it. Laptop mic noise floors measure ~0.0005-0.003;
/// quiet speech at arm's length is still >0.02.
pub const SILENCE_RMS: f32 = 0.006;

pub struct Recorder {
    buffer: Arc<Mutex<Vec<f32>>>,
    stream: Option<Stream>,
    sample_rate: u32,
    channels: u16,
    started_at: Option<Instant>,
    device: Option<String>,
}

/// Names of the microphones currently available.
pub fn input_device_names() -> Vec<String> {
    let host = cpal::default_host();
    let mut names: Vec<String> = host
        .input_devices()
        .map(|devs| devs.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default();
    names.sort();
    names.dedup();
    names
}

/// The device matching `preferred` (exact, then case-insensitive substring),
/// else the system default. Re-resolved on every dictation so plugging a
/// headset in or out takes effect immediately.
pub fn resolve_input_device(preferred: Option<&str>) -> Option<cpal::Device> {
    let host = cpal::default_host();
    if let Some(want) = preferred.map(str::trim).filter(|w| !w.is_empty()) {
        let want_lower = want.to_lowercase();
        if let Ok(devices) = host.input_devices() {
            let devices: Vec<cpal::Device> = devices.collect();
            let exact = devices
                .iter()
                .position(|d| d.name().is_ok_and(|n| n == want));
            let partial = || {
                devices.iter().position(|d| {
                    d.name()
                        .is_ok_and(|n| n.to_lowercase().contains(&want_lower))
                })
            };
            if let Some(i) = exact.or_else(partial) {
                return devices.into_iter().nth(i);
            }
        }
        tracing::warn!("microphone {want:?} not found; using the system default");
    }
    host.default_input_device()
}

impl Recorder {
    pub fn new() -> Result<Self> {
        Ok(Self {
            buffer: Arc::new(Mutex::new(Vec::new())),
            stream: None,
            sample_rate: 0,
            channels: 1,
            started_at: None,
            device: None,
        })
    }

    /// Microphone to use from the next `start()` on (None = system default).
    pub fn set_device(&mut self, device: Option<String>) {
        self.device = device;
    }

    pub fn is_recording(&self) -> bool {
        self.stream.is_some()
    }

    /// Time since `start()`, if recording.
    pub fn elapsed(&self) -> Option<Duration> {
        self.started_at.map(|t| t.elapsed())
    }

    pub fn start(&mut self) -> Result<()> {
        if self.stream.is_some() {
            return Ok(());
        }

        // A fresh buffer per dictation, never a cleared shared one: a
        // `LiveTap` from the previous dictation may still be draining its
        // audio on a worker thread, and must not see this one's samples.
        self.buffer = Arc::new(Mutex::new(Vec::new()));

        let device = resolve_input_device(self.device.as_deref())
            .ok_or_else(|| VoxError::Audio("no microphone found".into()))?;
        let config = device
            .default_input_config()
            .map_err(|e| VoxError::Audio(e.to_string()))?;

        self.sample_rate = config.sample_rate().0;
        self.channels = config.channels();
        let channels = self.channels as usize;
        let max_samples = self.sample_rate as usize * MAX_RECORDING.as_secs() as usize;
        let buffer = Arc::clone(&self.buffer);
        let stream_config: StreamConfig = config.clone().into();

        let stream = match config.sample_format() {
            SampleFormat::F32 => {
                build_stream::<f32>(&device, &stream_config, buffer, channels, max_samples)?
            }
            SampleFormat::I16 => {
                build_stream::<i16>(&device, &stream_config, buffer, channels, max_samples)?
            }
            SampleFormat::U16 => {
                build_stream::<u16>(&device, &stream_config, buffer, channels, max_samples)?
            }
            SampleFormat::I32 => {
                build_stream::<i32>(&device, &stream_config, buffer, channels, max_samples)?
            }
            SampleFormat::U8 => {
                build_stream::<u8>(&device, &stream_config, buffer, channels, max_samples)?
            }
            other => {
                return Err(VoxError::Audio(format!(
                    "unsupported sample format: {other:?}"
                )));
            }
        };

        stream.play().map_err(|e| VoxError::Audio(e.to_string()))?;
        self.stream = Some(stream);
        self.started_at = Some(Instant::now());
        Ok(())
    }

    /// Stops capture and discards the audio (Esc / cancel).
    pub fn cancel(&mut self) {
        self.stream.take();
        self.started_at = None;
        lock_buffer(&self.buffer).clear();
    }

    /// A live view of the audio being captured right now, for transcribing
    /// while the user is still speaking. Only meaningful after `start()`;
    /// it keeps seeing this dictation's samples after `stop_capture()`.
    pub fn tap(&self) -> LiveTap {
        LiveTap {
            buffer: Arc::clone(&self.buffer),
            sample_rate: if self.sample_rate == 0 {
                TARGET_RATE
            } else {
                self.sample_rate
            },
        }
    }

    /// Stops capture but leaves the audio where a `LiveTap` can read it
    /// (the streaming path owns the samples from here on).
    pub fn stop_capture(&mut self) {
        self.stream.take();
        self.started_at = None;
    }

    pub fn stop(&mut self) -> Result<AudioChunk> {
        self.stream.take(); // drop stops capture
        self.started_at = None;
        let samples = {
            let mut buf = lock_buffer(&self.buffer);
            std::mem::take(&mut *buf)
        };
        let rate = if self.sample_rate == 0 {
            TARGET_RATE
        } else {
            self.sample_rate
        };
        Ok(finish_capture(&samples, rate))
    }
}

/// Raw mono capture at `rate` -> trimmed 16 kHz audio ready for Whisper.
pub fn finish_capture(samples: &[f32], rate: u32) -> AudioChunk {
    let trimmed = trim_silence(samples, rate);
    let resampled = resample(&trimmed, rate, TARGET_RATE);
    AudioChunk {
        samples: resampled,
        sample_rate: TARGET_RATE,
    }
}

/// Shared, growing view of one dictation's raw mono capture (at the device's
/// native rate). The audio callback appends; readers copy out what's new.
#[derive(Clone)]
pub struct LiveTap {
    buffer: Arc<Mutex<Vec<f32>>>,
    sample_rate: u32,
}

impl LiveTap {
    /// An empty tap fed by `push` instead of a microphone (file playback in
    /// `vox transcribe --stream`, and tests).
    pub fn new(sample_rate: u32) -> Self {
        Self {
            buffer: Arc::new(Mutex::new(Vec::new())),
            sample_rate,
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn push(&self, samples: &[f32]) {
        lock_buffer(&self.buffer).extend_from_slice(samples);
    }

    pub fn len(&self) -> usize {
        lock_buffer(&self.buffer).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Appends every sample from index `from` on to `out`, returning the
    /// new read position. The lock is held only for the copy.
    pub fn read_from(&self, from: usize, out: &mut Vec<f32>) -> usize {
        let buf = lock_buffer(&self.buffer);
        if from < buf.len() {
            out.extend_from_slice(&buf[from..]);
        }
        buf.len().max(from)
    }
}

fn lock_buffer(buffer: &Arc<Mutex<Vec<f32>>>) -> std::sync::MutexGuard<'_, Vec<f32>> {
    buffer.lock().unwrap_or_else(|p| p.into_inner())
}

pub(crate) trait SampleToF32: cpal::SizedSample + Copy {
    fn to_f32(self) -> f32;
}

impl SampleToF32 for f32 {
    fn to_f32(self) -> f32 {
        self
    }
}

impl SampleToF32 for i16 {
    fn to_f32(self) -> f32 {
        self as f32 / i16::MAX as f32
    }
}

impl SampleToF32 for u16 {
    fn to_f32(self) -> f32 {
        (self as f32 / u16::MAX as f32) * 2.0 - 1.0
    }
}

impl SampleToF32 for i32 {
    fn to_f32(self) -> f32 {
        self as f32 / i32::MAX as f32
    }
}

impl SampleToF32 for u8 {
    fn to_f32(self) -> f32 {
        (self as f32 / u8::MAX as f32) * 2.0 - 1.0
    }
}

fn build_stream<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    buffer: Arc<Mutex<Vec<f32>>>,
    channels: usize,
    max_samples: usize,
) -> Result<Stream>
where
    T: SampleToF32 + Send + 'static,
{
    let err_fn = |err| eprintln!("audio stream error: {err}");
    device
        .build_input_stream(
            config,
            move |data: &[T], _| {
                if channels == 0 {
                    return;
                }
                let mut buf = lock_buffer(&buffer);
                if buf.len() >= max_samples {
                    return;
                }
                buf.extend(
                    data.chunks(channels).map(|frame| {
                        frame.iter().map(|s| s.to_f32()).sum::<f32>() / channels as f32
                    }),
                );
            },
            err_fn,
            None,
        )
        .map_err(|e| VoxError::Audio(e.to_string()))
}

pub(crate) fn window_len(sample_rate: u32) -> usize {
    ((sample_rate as u64 * WINDOW_MS as u64) / 1000) as usize
}

pub(crate) fn window_rms(samples: &[f32], sample_rate: u32) -> Vec<f32> {
    let window_len = ((sample_rate as u64 * WINDOW_MS as u64) / 1000) as usize;
    if window_len == 0 {
        return Vec::new();
    }
    samples
        .chunks(window_len)
        .map(|chunk| (chunk.iter().map(|s| s * s).sum::<f32>() / chunk.len() as f32).sqrt())
        .collect()
}

/// True when no 20 ms window rises above the silence floor.
pub fn is_silent(samples: &[f32], sample_rate: u32) -> bool {
    window_rms(samples, sample_rate)
        .into_iter()
        .fold(0.0f32, f32::max)
        < SILENCE_RMS
}

pub fn trim_silence(samples: &[f32], sample_rate: u32) -> Vec<f32> {
    if samples.is_empty() || sample_rate == 0 {
        return samples.to_vec();
    }

    let window_len = ((sample_rate as u64 * WINDOW_MS as u64) / 1000) as usize;
    if window_len == 0 {
        return samples.to_vec();
    }

    let window_rms = window_rms(samples, sample_rate);
    if window_rms.is_empty() {
        return samples.to_vec();
    }

    // 95th-percentile (nearest-rank), not max
    let mut sorted = window_rms.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let rank = ((PEAK_PERCENTILE * sorted.len() as f32).ceil() as usize)
        .saturating_sub(1)
        .min(sorted.len() - 1);
    let peak = sorted[rank];

    if peak <= f32::EPSILON {
        return samples.to_vec();
    }

    let threshold = peak * THRESHOLD_RATIO;
    let first = window_rms.iter().position(|&r| r >= threshold);
    let last = window_rms.iter().rposition(|&r| r >= threshold);
    let (Some(first), Some(last)) = (first, last) else {
        return samples.to_vec();
    };

    let padding = ((sample_rate as u64 * PADDING_MS as u64) / 1000) as usize;
    let start = (first * window_len).saturating_sub(padding);
    let end = ((last + 1) * window_len + padding).min(samples.len());
    samples[start..end].to_vec()
}

/// Band-limited (windowed-sinc) sample-rate conversion.
///
/// Mics run at 44.1/48 kHz and Whisper wants 16 kHz. Plain linear
/// interpolation has no anti-aliasing filter, so everything between 8 kHz and
/// 24 kHz (sibilants, fan noise, keyboard clicks) folds back into the speech
/// band. This low-passes at the target Nyquist while interpolating, using a
/// precomputed polyphase kernel (one phase for 48k->16k, 160 for 44.1k->16k),
/// so it costs ~50 multiply-adds per output sample: ~1 ms per second of audio.
pub fn resample(samples: &[f32], src: u32, dst: u32) -> Vec<f32> {
    if samples.is_empty() || src == 0 || dst == 0 || src == dst {
        return samples.to_vec();
    }

    const ZERO_CROSSINGS: f64 = 8.0;
    const MAX_PHASES: u64 = 1024;

    let g = gcd(src as u64, dst as u64);
    let up = dst as u64 / g; // output i sits at input position i * down / up
    let down = src as u64 / g;
    let phases = up.min(MAX_PHASES);

    let ratio = dst as f64 / src as f64;
    // Cutoff relative to the source Nyquist, just under the target Nyquist.
    let cutoff = ratio.min(1.0) * 0.94;
    let half_width = (ZERO_CROSSINGS / cutoff).ceil() as isize;
    let taps = (2 * half_width) as usize;

    // kernel[p * taps + (j + half_width - 1)] for fractional offset p/phases
    // and tap j in (-half_width, half_width], each phase normalized to unity
    // DC gain.
    let mut kernel = vec![0.0f32; phases as usize * taps];
    for p in 0..phases as usize {
        let frac = p as f64 / phases as f64;
        let row = &mut kernel[p * taps..(p + 1) * taps];
        let mut sum = 0.0f64;
        let mut weights = vec![0.0f64; taps];
        for (idx, w) in weights.iter_mut().enumerate() {
            let j = idx as isize - half_width + 1;
            let d = frac - j as f64;
            *w = blackman(d / (half_width as f64 + 1.0)) * sinc(d * cutoff);
            sum += *w;
        }
        for (r, w) in row.iter_mut().zip(&weights) {
            *r = (w / sum) as f32;
        }
    }

    let out_len = ((samples.len() as f64) * ratio).round() as usize;
    let last = samples.len() as isize - 1;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len as u64 {
        let pos = i * down;
        let center = (pos / up) as isize;
        let phase = ((pos % up) * phases / up) as usize;
        let row = &kernel[phase * taps..(phase + 1) * taps];
        let first = center - half_width + 1;
        let acc: f32 = if first >= 0 && first + taps as isize - 1 <= last {
            let window = &samples[first as usize..first as usize + taps];
            window.iter().zip(row).map(|(s, k)| s * k).sum()
        } else {
            row.iter()
                .enumerate()
                .map(|(idx, k)| samples[(first + idx as isize).clamp(0, last) as usize] * k)
                .sum()
        };
        out.push(acc);
    }
    out
}

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a.max(1)
}

fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-9 {
        1.0
    } else {
        let px = std::f64::consts::PI * x;
        px.sin() / px
    }
}

/// Blackman window over x in [-1, 1].
fn blackman(x: f64) -> f64 {
    if x.abs() >= 1.0 {
        return 0.0;
    }
    let n = (x + 1.0) / 2.0; // 0..1
    let tau = 2.0 * std::f64::consts::PI;
    0.42 - 0.5 * (tau * n).cos() + 0.08 * (2.0 * tau * n).cos()
}

pub fn record_for(seconds: f32, device: Option<String>) -> Result<AudioChunk> {
    let mut recorder = Recorder::new()?;
    recorder.set_device(device);
    recorder.start()?;
    thread::sleep(Duration::from_secs_f32(seconds.max(0.0)));
    recorder.stop()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, rate: u32, secs: f32, amp: f32) -> Vec<f32> {
        let n = (rate as f32 * secs) as usize;
        (0..n)
            .map(|i| amp * (2.0 * std::f32::consts::PI * freq * i as f32 / rate as f32).sin())
            .collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|s| s * s).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    #[test]
    fn trim_silence_keeps_quiet_speech_after_loud_spike() {
        let rate = 16_000u32;
        let window = (rate as usize * WINDOW_MS as usize) / 1000;
        // Loud spike in first window, quiet speech later
        let mut samples = vec![0.0f32; window * 20];
        for s in &mut samples[0..window] {
            *s = 1.0; // loud spike
        }
        // Quiet speech in windows 10-14
        for s in &mut samples[window * 10..window * 15] {
            *s = 0.15;
        }
        let trimmed = trim_silence(&samples, rate);
        // Must retain the quiet speech region (not clipped away by spike-based threshold)
        assert!(trimmed.len() > window * 3);
        let peak_in_trim = trimmed.iter().cloned().fold(0.0f32, f32::max);
        assert!(peak_in_trim >= 0.14);
    }

    #[test]
    fn resample_identity_and_upsample() {
        let samples = vec![0.0, 1.0, 0.0, -1.0];
        assert_eq!(resample(&samples, 16000, 16000), samples);
        let up = resample(&samples, 8000, 16000);
        assert_eq!(up.len(), 8);
    }

    #[test]
    fn resample_preserves_in_band_tone() {
        let input = sine(1000.0, 48_000, 1.0, 0.5);
        let out = resample(&input, 48_000, 16_000);
        assert_eq!(out.len(), 16_000);
        // skip filter edges
        let mid = &out[1000..15_000];
        let expected = 0.5 / 2f32.sqrt();
        assert!((rms(mid) - expected).abs() < 0.02, "rms {}", rms(mid));
    }

    #[test]
    fn resample_rejects_out_of_band_tone_instead_of_aliasing() {
        // 12 kHz is above the 8 kHz Nyquist of 16 kHz audio; linear
        // interpolation folds it to 4 kHz at nearly full strength.
        let input = sine(12_000.0, 48_000, 1.0, 0.5);
        let out = resample(&input, 48_000, 16_000);
        let mid = &out[1000..15_000];
        assert!(rms(mid) < 0.01, "aliased energy {}", rms(mid));
    }

    #[test]
    fn resample_44100_has_expected_length() {
        let input = sine(440.0, 44_100, 2.0, 0.3);
        let out = resample(&input, 44_100, 16_000);
        assert_eq!(out.len(), 32_000);
    }

    #[test]
    fn silence_gate() {
        let rate = 16_000;
        let mut noise: Vec<f32> = (0..rate)
            .map(|i| ((i * 7919 % 1000) as f32 / 1000.0 - 0.5) * 0.004)
            .collect();
        assert!(is_silent(&noise, rate));
        let speech = sine(300.0, rate, 0.3, 0.1);
        noise.extend(speech);
        assert!(!is_silent(&noise, rate));
        assert!(is_silent(&[], rate));
    }
}
