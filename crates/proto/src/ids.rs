use serde::{Deserialize, Serialize};

/// Server-assigned participant id, unique for the lifetime of the server process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ParticipantId(pub u64);

/// Server-assigned playlist item id. Monotonic, persisted across restarts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ItemId(pub u64);

/// Library file id: the first 16 hex characters of `blake3(root_name + "/" + relative_path)`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FileId(pub String);

/// Room id. Stage 1 always uses [`RoomId::main`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RoomId(pub String);

impl RoomId {
    pub fn main() -> Self {
        RoomId("main".to_owned())
    }
}

/// Client-chosen request id echoed back in `Denied` and `Playback.cause`.
pub type ReqId = u32;
