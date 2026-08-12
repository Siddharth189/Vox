//! Vox — local-first macOS menu-bar voice dictation.

pub mod audio;
pub mod config;
pub mod context;
pub mod doctor;
pub mod error;
pub mod history;
pub mod inject;
pub mod model;
pub mod permissions;
pub mod pipeline;
pub mod privacy;
pub mod process;
pub mod secure_fs;
pub mod settings_web;
pub mod stt;
pub mod tray;

pub use error::{Result, VoxError};
pub use model::*;
