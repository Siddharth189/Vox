use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};

use crate::error::{Result, VoxError};
use crate::model::AudioChunk;

pub const TARGET_RATE: u32 = 16_000;

const WINDOW_MS: u32 = 20;
const PADDING_MS: u32 = 100;
const THRESHOLD_RATIO: f32 = 0.1;
const PEAK_PERCENTILE: f32 = 0.95;

pub struct Recorder {
    buffer: Arc<Mutex<Vec<f32>>>,
    stream: Option<Stream>,
    sample_rate: u32,
    channels: u16,
}

impl Recorder {
    pub fn new() -> Result<Self> {
        Ok(Self {
            buffer: Arc::new(Mutex::new(Vec::new())),
            stream: None,
            sample_rate: 0,
            channels: 1,
        })
    }

    pub fn start(&mut self) -> Result<()> {
        if self.stream.is_some() {
            return Ok(());
        }

        {
            let mut buf = lock_buffer(&self.buffer);
            buf.clear();
        }

        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| VoxError::Audio("no default input device".into()))?;
        let config = device
            .default_input_config()
            .map_err(|e| VoxError::Audio(e.to_string()))?;

        self.sample_rate = config.sample_rate().0;
        self.channels = config.channels();
        let channels = self.channels as usize;
        let buffer = Arc::clone(&self.buffer);
        let stream_config: StreamConfig = config.clone().into();

        let stream = match config.sample_format() {
            SampleFormat::F32 => build_stream::<f32>(&device, &stream_config, buffer, channels)?,
            SampleFormat::I16 => build_stream::<i16>(&device, &stream_config, buffer, channels)?,
            SampleFormat::U16 => build_stream::<u16>(&device, &stream_config, buffer, channels)?,
            other => {
                return Err(VoxError::Audio(format!(
                    "unsupported sample format: {other:?}"
                )));
            }
        };

        stream
            .play()
            .map_err(|e| VoxError::Audio(e.to_string()))?;
        self.stream = Some(stream);
        Ok(())
    }

    pub fn stop(&mut self) -> Result<AudioChunk> {
        self.stream.take(); // drop stops capture
        let samples = {
            let mut buf = lock_buffer(&self.buffer);
            std::mem::take(&mut *buf)
        };
        let rate = if self.sample_rate == 0 {
            TARGET_RATE
        } else {
            self.sample_rate
        };
        let trimmed = trim_silence(&samples, rate);
        let resampled = resample_linear(&trimmed, rate, TARGET_RATE);
        Ok(AudioChunk {
            samples: resampled,
            sample_rate: TARGET_RATE,
        })
    }
}

fn lock_buffer(buffer: &Arc<Mutex<Vec<f32>>>) -> std::sync::MutexGuard<'_, Vec<f32>> {
    buffer
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

trait SampleToF32: cpal::SizedSample + Copy {
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

fn build_stream<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    buffer: Arc<Mutex<Vec<f32>>>,
    channels: usize,
) -> Result<Stream>
where
    T: SampleToF32 + Send + 'static,
{
    let err_fn = |err| eprintln!("audio stream error: {err}");
    device
        .build_input_stream(
            config,
            move |data: &[T], _| {
                let mut mono = Vec::with_capacity(data.len() / channels.max(1));
                if channels == 0 {
                    return;
                }
                for frame in data.chunks(channels) {
                    let sum: f32 = frame.iter().map(|s| s.to_f32()).sum();
                    mono.push(sum / channels as f32);
                }
                let mut buf = lock_buffer(&buffer);
                buf.extend_from_slice(&mono);
            },
            err_fn,
            None,
        )
        .map_err(|e| VoxError::Audio(e.to_string()))
}

pub fn trim_silence(samples: &[f32], sample_rate: u32) -> Vec<f32> {
    if samples.is_empty() || sample_rate == 0 {
        return samples.to_vec();
    }

    let window_len = ((sample_rate as u64 * WINDOW_MS as u64) / 1000) as usize;
    if window_len == 0 {
        return samples.to_vec();
    }

    let mut window_rms = Vec::new();
    for chunk in samples.chunks(window_len) {
        let sum_sq: f32 = chunk.iter().map(|s| s * s).sum();
        let rms = (sum_sq / chunk.len() as f32).sqrt();
        window_rms.push(rms);
    }

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

pub fn resample_linear(samples: &[f32], src: u32, dst: u32) -> Vec<f32> {
    if samples.is_empty() || src == 0 || dst == 0 || src == dst {
        return samples.to_vec();
    }

    let out_len = ((samples.len() as f64) * (dst as f64) / (src as f64)).round() as usize;
    if out_len == 0 {
        return Vec::new();
    }

    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src_pos = (i as f64) * (src as f64) / (dst as f64);
        let idx = src_pos.floor() as usize;
        let frac = (src_pos - idx as f64) as f32;
        let a = samples[idx.min(samples.len() - 1)];
        let b = samples[(idx + 1).min(samples.len() - 1)];
        out.push(a + (b - a) * frac);
    }
    out
}

pub fn record_for(seconds: f32) -> Result<AudioChunk> {
    let mut recorder = Recorder::new()?;
    recorder.start()?;
    thread::sleep(Duration::from_secs_f32(seconds.max(0.0)));
    recorder.stop()
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(resample_linear(&samples, 16000, 16000), samples);
        let up = resample_linear(&samples, 8000, 16000);
        assert_eq!(up.len(), 8);
    }
}
