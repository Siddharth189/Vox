//! Vox - local-first voice dictation for macOS, Windows, and Linux.

pub mod audio;
pub mod autostart;
pub mod config;
pub mod context;
pub mod doctor;
pub mod error;
pub mod history;
pub mod hotkey;
pub mod inject;
pub mod model;
pub mod models;
pub mod permissions;
pub mod pipeline;
pub mod privacy;
pub mod process;
pub mod secure_fs;
pub mod settings_web;
pub mod speed;
pub mod stats;
pub mod stt;
pub mod tray;
pub mod util;

pub use error::{Result, VoxError};
pub use model::*;
