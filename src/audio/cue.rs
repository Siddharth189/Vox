//! Short synthesized start/stop tones, so you know Vox is listening without
//! looking at the tray (which Windows hides in the overflow by default).
//! Generated in code: no audio assets in the bundle.

use std::thread;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cue {
    /// Rising two-note chirp: listening.
    Start,
    /// Falling two-note chirp: got it, transcribing.
    Stop,
    /// Single low blip: cancelled, or nothing was heard.
    Cancel,
}

const VOLUME: f32 = 0.18;

impl Cue {
    /// (frequency Hz, duration ms) segments.
    fn notes(self) -> &'static [(f32, u32)] {
        match self {
            Cue::Start => &[(660.0, 55), (990.0, 70)],
            Cue::Stop => &[(990.0, 55), (660.0, 70)],
            Cue::Cancel => &[(330.0, 120)],
        }
    }
}

/// Plays `cue` on the default output device without blocking the caller.
pub fn play(cue: Cue) {
    thread::spawn(move || {
        if let Err(e) = play_blocking(cue) {
            tracing::debug!("sound cue failed: {e}");
        }
    });
}

pub fn render(cue: Cue, sample_rate: u32) -> Vec<f32> {
    let mut out = Vec::new();
    let fade = (sample_rate as f32 * 0.006) as usize; // 6 ms ramps: no clicks
    for &(freq, ms) in cue.notes() {
        let n = (sample_rate as u64 * ms as u64 / 1000) as usize;
        for i in 0..n {
            let ramp_in = (i as f32 / fade.max(1) as f32).min(1.0);
            let ramp_out = ((n - i) as f32 / fade.max(1) as f32).min(1.0);
            let phase = 2.0 * std::f32::consts::PI * freq * i as f32 / sample_rate as f32;
            out.push(VOLUME * ramp_in * ramp_out * phase.sin());
        }
    }
    out
}

fn play_blocking(cue: Cue) -> Result<(), String> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| "no output device".to_string())?;
    let config = device.default_output_config().map_err(|e| e.to_string())?;
    let rate = config.sample_rate().0;
    let channels = config.channels() as usize;
    let samples = render(cue, rate);
    let duration = Duration::from_millis(samples.len() as u64 * 1000 / rate as u64);
    let stream_config: cpal::StreamConfig = config.clone().into();

    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => build::<f32>(&device, &stream_config, samples, channels),
        cpal::SampleFormat::I16 => build::<i16>(&device, &stream_config, samples, channels),
        cpal::SampleFormat::U16 => build::<u16>(&device, &stream_config, samples, channels),
        cpal::SampleFormat::I32 => build::<i32>(&device, &stream_config, samples, channels),
        other => return Err(format!("unsupported output format {other:?}")),
    }?;
    stream.play().map_err(|e| e.to_string())?;
    // Let the device drain its buffer before the stream is dropped.
    thread::sleep(duration + Duration::from_millis(120));
    Ok(())
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    samples: Vec<f32>,
    channels: usize,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + FromSample<f32> + Send + 'static,
{
    let mut pos = 0usize;
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _| {
                for frame in data.chunks_mut(channels.max(1)) {
                    let s = samples.get(pos).copied().unwrap_or(0.0);
                    pos += 1;
                    for out in frame.iter_mut() {
                        *out = T::from_sample(s);
                    }
                }
            },
            |e| tracing::debug!("cue stream error: {e}"),
            None,
        )
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cues_are_short_quiet_and_click_free() {
        for cue in [Cue::Start, Cue::Stop, Cue::Cancel] {
            let s = render(cue, 48_000);
            assert!(!s.is_empty());
            assert!(s.len() < 48_000 / 4, "cue longer than 250 ms");
            let peak = s.iter().fold(0.0f32, |m, x| m.max(x.abs()));
            assert!(peak <= VOLUME + 1e-6);
            assert!(s[0].abs() < 0.01 && s[s.len() - 1].abs() < 0.01);
        }
    }
}
