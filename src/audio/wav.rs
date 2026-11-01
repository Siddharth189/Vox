//! Minimal RIFF/WAVE reader (PCM 8/16/24/32-bit and IEEE float), enough for
//! `vox transcribe file.wav` without pulling in a codec crate.

use std::path::Path;

use crate::audio::finish_capture;
use crate::error::{Result, VoxError};
use crate::model::AudioChunk;

/// Decoded mono samples at the file's native rate.
pub struct Wav {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
}

pub fn read(path: &Path) -> Result<Wav> {
    let bytes = std::fs::read(path)?;
    parse(&bytes)
}

/// Reads `path` and converts it exactly like a live recording.
pub fn read_as_capture(path: &Path) -> Result<AudioChunk> {
    let wav = read(path)?;
    Ok(finish_capture(&wav.samples, wav.sample_rate))
}

fn bad(msg: &str) -> VoxError {
    VoxError::Audio(format!("unsupported WAV file: {msg}"))
}

fn u16_at(b: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(i..i + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?))
}

pub fn parse(bytes: &[u8]) -> Result<Wav> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(bad("not RIFF/WAVE"));
    }
    let mut fmt: Option<(u16, u16, u32, u16)> = None; // format, channels, rate, bits
    let mut data: Option<&[u8]> = None;
    let mut i = 12;
    while i + 8 <= bytes.len() {
        let id = &bytes[i..i + 4];
        let len = u32_at(bytes, i + 4).ok_or_else(|| bad("truncated chunk"))? as usize;
        let body_start = i + 8;
        let body_end = body_start.saturating_add(len).min(bytes.len());
        let body = &bytes[body_start..body_end];
        match id {
            b"fmt " => {
                let mut format = u16_at(body, 0).ok_or_else(|| bad("short fmt"))?;
                let channels = u16_at(body, 2).ok_or_else(|| bad("short fmt"))?;
                let rate = u32_at(body, 4).ok_or_else(|| bad("short fmt"))?;
                let bits = u16_at(body, 14).ok_or_else(|| bad("short fmt"))?;
                if format == 0xFFFE {
                    // WAVE_FORMAT_EXTENSIBLE: real format is the subformat GUID's first u16
                    format = u16_at(body, 24).ok_or_else(|| bad("short extensible fmt"))?;
                }
                fmt = Some((format, channels, rate, bits));
            }
            b"data" => data = Some(body),
            _ => {}
        }
        i = body_start + len + (len & 1);
    }
    let (format, channels, rate, bits) = fmt.ok_or_else(|| bad("no fmt chunk"))?;
    let data = data.ok_or_else(|| bad("no data chunk"))?;
    if channels == 0 || rate == 0 {
        return Err(bad("zero channels or sample rate"));
    }

    let frame_values: Vec<f32> = match (format, bits) {
        (1, 8) => data.iter().map(|&b| (b as f32 - 128.0) / 128.0).collect(),
        (1, 16) => data
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| i16::from_le_bytes(*c) as f32 / 32768.0)
            .collect(),
        (1, 24) => data
            .as_chunks::<3>()
            .0
            .iter()
            .map(|c| (i32::from_le_bytes([0, c[0], c[1], c[2]]) >> 8) as f32 / 8_388_608.0)
            .collect(),
        (1, 32) => data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| i32::from_le_bytes(*c) as f32 / 2_147_483_648.0)
            .collect(),
        (3, 32) => data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_le_bytes(*c))
            .collect(),
        _ => return Err(bad(&format!("format {format} with {bits}-bit samples"))),
    };

    let ch = channels as usize;
    let samples = frame_values
        .chunks_exact(ch)
        .map(|f| f.iter().sum::<f32>() / ch as f32)
        .collect();
    Ok(Wav {
        samples,
        sample_rate: rate,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pcm16_wav(rate: u32, channels: u16, samples: &[i16]) -> Vec<u8> {
        let data_len = (samples.len() * 2) as u32;
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data_len).to_le_bytes());
        b.extend_from_slice(b"WAVE");
        b.extend_from_slice(b"fmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&channels.to_le_bytes());
        b.extend_from_slice(&rate.to_le_bytes());
        b.extend_from_slice(&(rate * channels as u32 * 2).to_le_bytes());
        b.extend_from_slice(&(channels * 2).to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"LIST");
        b.extend_from_slice(&3u32.to_le_bytes()); // odd-length chunk + pad byte
        b.extend_from_slice(&[1, 2, 3, 0]);
        b.extend_from_slice(b"data");
        b.extend_from_slice(&data_len.to_le_bytes());
        for s in samples {
            b.extend_from_slice(&s.to_le_bytes());
        }
        b
    }

    #[test]
    fn parses_stereo_pcm16_and_downmixes() {
        let bytes = pcm16_wav(22_050, 2, &[16384, 0, -16384, -16384]);
        let wav = parse(&bytes).unwrap();
        assert_eq!(wav.sample_rate, 22_050);
        assert_eq!(wav.samples.len(), 2);
        assert!((wav.samples[0] - 0.25).abs() < 1e-3);
        assert!((wav.samples[1] + 0.5).abs() < 1e-3);
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse(b"not a wav file at all").is_err());
        assert!(parse(&[]).is_err());
    }
}
