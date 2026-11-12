//! Snapshot of the room that survives a server restart (spec §4.8).

use serde::{Deserialize, Serialize};
use sppy_proto::{ControlMode, Item, ItemId};

use super::{Room, RoomConfig};

pub const PERSISTED_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersistedState {
    pub version: u32,
    pub playlist: Vec<Item>,
    pub next_item_id: u64,
    pub current: Option<ItemId>,
    pub position_ms: u64,
    pub control_mode: ControlMode,
}

impl Room {
    pub fn persisted(&self, now_ms: u64) -> PersistedState {
        PersistedState {
            version: PERSISTED_VERSION,
            playlist: self.playlist.clone(),
            next_item_id: self.next_item_id,
            current: self.playback.item_id,
            position_ms: self.playback.expected_position_ms(now_ms),
            control_mode: self.control_mode,
        }
    }

    /// Rebuild a room after a restart: same playlist, paused at the saved position, nobody in it.
    pub fn restore(config: RoomConfig, state: PersistedState) -> Room {
        let mut room = Room::new(config);
        let current = state.current.filter(|id| state.playlist.iter().any(|i| i.item_id == *id));
        room.playlist = state.playlist;
        room.next_item_id = state.next_item_id;
        room.control_mode = state.control_mode;
        room.playback.item_id = current;
        room.playback.position_ms = if current.is_some() { state.position_ms } else { 0 };
        room.playback.paused = true;
        room
    }
}
