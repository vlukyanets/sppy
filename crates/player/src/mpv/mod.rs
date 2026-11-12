//! mpv backend: JSON IPC over a Unix socket / Windows named pipe.

pub mod ipc;
mod player;
pub mod wire;

pub use player::{MpvConfig, MpvLauncher, MpvPlayer};
