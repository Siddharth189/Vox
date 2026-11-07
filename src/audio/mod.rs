pub mod cue;
mod recorder;
pub mod wav;

pub use recorder::{
    finish_capture, input_device_names, is_silent, record_for, resample, resolve_input_device,
    trim_silence, LiveTap, Recorder, MAX_RECORDING, SILENCE_RMS, TARGET_RATE,
};
pub(crate) use recorder::{window_len, window_rms};
