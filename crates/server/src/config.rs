//! Server configuration (shared type lives in `sppy-common`).

pub use sppy_common::config::{MediaRoot, ServerConfig};
use sppy_proto::RoomSettings;

use crate::room::RoomConfig;

/// Room parameters derived from the server configuration.
pub fn room_config(cfg: &ServerConfig) -> RoomConfig {
    RoomConfig {
        max_participants: cfg.max_participants,
        resume_grace_ms: cfg.resume_grace_secs * 1000,
        settings: RoomSettings {
            stall_hold_secs: cfg.stall_hold_secs,
            resume_buffer_secs: cfg.resume_buffer_secs,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn room_config_from_defaults() {
        let rc = room_config(&ServerConfig::default());
        assert_eq!(rc.max_participants, 8);
        assert_eq!(rc.resume_grace_ms, 60_000);
        assert_eq!(rc.settings, RoomSettings { stall_hold_secs: 10, resume_buffer_secs: 10 });
    }
}
