use serde::{Deserialize, Serialize};

use crate::ids::{ItemId, ParticipantId};
use crate::state::{ControlMode, Hold};

/// Structured notification; clients render the text themselves.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum NoticeAction {
    Paused,
    Resumed,
    Seeked { to_ms: u64 },
    SpeedChanged { speed: f64 },
    ItemChanged { item_id: ItemId },
    PlaylistAdded { item_ids: Vec<ItemId> },
    PlaylistRemoved { item_id: ItemId },
    PlaylistMoved { item_id: ItemId },
    Joined,
    Left,
    Kicked { participant_id: ParticipantId },
    ControlModeChanged { mode: ControlMode },
    CanControlChanged { participant_id: ParticipantId, allowed: bool },
    HoldStarted { hold: Hold },
    HoldEnded,
    ItemSkipped { item_id: ItemId, reason: SkipReason },
    DeliveryFellBack { participant_id: ParticipantId },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SkipReason {
    FileMissing,
    AllFailed,
}
