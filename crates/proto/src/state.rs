use serde::{Deserialize, Serialize};

use crate::ids::{ItemId, ParticipantId, ReqId, RoomId};
use crate::library::{Fingerprint, Subtitle};

/// Server-authoritative playback state (spec §6.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlaybackState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<ItemId>,
    pub position_ms: u64,
    pub at_server_ms: u64,
    /// Human intent.
    pub paused: bool,
    pub speed: f64,
    /// System block (waiting for participants).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hold: Option<Hold>,
    pub seq: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause: Option<Cause>,
}

impl PlaybackState {
    /// The video plays only when nobody paused it and the server is not holding the room.
    pub fn is_playing(&self) -> bool {
        !self.paused && self.hold.is_none()
    }

    /// Position the room should be at, at server time `now_server_ms`.
    pub fn expected_position_ms(&self, now_server_ms: u64) -> u64 {
        if !self.is_playing() {
            return self.position_ms;
        }
        let elapsed = now_server_ms.saturating_sub(self.at_server_ms) as f64;
        self.position_ms.saturating_add((elapsed * self.speed).round() as u64)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hold {
    pub reason: HoldReason,
    pub waiting_for: Vec<ParticipantId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HoldReason {
    ItemStart,
    Stalled,
}

/// Which request produced a playback change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cause {
    pub participant_id: ParticipantId,
    pub req_id: ReqId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoomState {
    pub room_id: RoomId,
    pub participants: Vec<Participant>,
    pub control_mode: ControlMode,
    pub playlist: Vec<Item>,
    pub settings: RoomSettings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomSettings {
    pub stall_hold_secs: u32,
    pub resume_buffer_secs: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Participant {
    pub id: ParticipantId,
    pub nickname: String,
    pub is_host: bool,
    pub can_control: bool,
    pub connected: bool,
    pub status: MediaStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ControlMode {
    Everyone,
    HostOnly,
    Selected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub item_id: ItemId,
    pub source: Source,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<Fingerprint>,
    pub subtitles: Vec<Subtitle>,
    pub added_by: ParticipantId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum Source {
    File { file_id: crate::ids::FileId },
    Url { url: String },
}

/// A participant's state for the current item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum MediaStatus {
    Loading,
    Ready {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    Stalled,
    Failed {
        message: String,
    },
    PlayerClosed,
}

/// How a participant gets the current item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum Delivery {
    Download { bytes: u64, total: u64 },
    Stream,
    Local,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgressEntry {
    pub participant_id: ParticipantId,
    pub item_id: ItemId,
    pub delivery: Delivery,
    /// The participant's last reported drift from the room position, in ms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drift_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub from: ParticipantId,
    pub nickname: String,
    pub text: String,
    pub at_server_ms: u64,
}
