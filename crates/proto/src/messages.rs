use serde::{Deserialize, Serialize};

use crate::ids::{ItemId, ParticipantId, ReqId, RoomId};
use crate::library::{FolderAdding, LibraryNode, MediaFolder};
use crate::notice::NoticeAction;
use crate::state::{
    ChatMessage, ControlMode, Delivery, LagPolicy, MediaStatus, PlaybackState, ProgressEntry,
    RoomState, Source,
};

/// How a client authenticates in `Hello`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum Auth {
    Host { token: String },
    Invite { token: String },
    Resume { session_token: String },
}

/// Client → server messages (spec §5.2–5.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClientMsg {
    Hello {
        protocol: u32,
        room: RoomId,
        auth: Auth,
        nickname: String,
        client: String,
    },
    Ping {
        client_time_ms: u64,
    },
    Bye,
    Play {
        req_id: ReqId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        position_ms: Option<u64>,
    },
    Pause {
        req_id: ReqId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        position_ms: Option<u64>,
    },
    Seek {
        req_id: ReqId,
        position_ms: u64,
    },
    SetSpeed {
        req_id: ReqId,
        speed: f64,
    },
    PlaylistAdd {
        req_id: ReqId,
        sources: Vec<Source>,
        /// Insert after this item; `None` appends to the end.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after: Option<ItemId>,
    },
    PlaylistRemove {
        req_id: ReqId,
        item_id: ItemId,
    },
    PlaylistMove {
        req_id: ReqId,
        item_id: ItemId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after: Option<ItemId>,
    },
    PlaylistSelect {
        req_id: ReqId,
        item_id: ItemId,
    },
    SetStatus {
        item_id: ItemId,
        status: MediaStatus,
    },
    /// At most once per second: how the item arrives and, while playing, the drift.
    Progress {
        item_id: ItemId,
        delivery: Delivery,
        /// Player position minus the room position, in ms.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        drift_ms: Option<i64>,
    },
    ItemEnded {
        item_id: ItemId,
    },
    Chat {
        text: String,
    },
    /// Ready (or no longer) for the current item to start.
    SetReady {
        ready: bool,
    },
    GetLibrary,
    SetControlMode {
        req_id: ReqId,
        mode: ControlMode,
    },
    /// Host: what the room does about participants who fall behind.
    SetLagPolicy {
        req_id: ReqId,
        policy: LagPolicy,
    },
    /// Host: whether a new item also waits for everyone to press Ready.
    SetReadyCheck {
        req_id: ReqId,
        on: bool,
    },
    SetCanControl {
        req_id: ReqId,
        participant_id: ParticipantId,
        allowed: bool,
    },
    Kick {
        req_id: ReqId,
        participant_id: ParticipantId,
    },
    RotateInvite {
        req_id: ReqId,
    },
    RescanLibrary {
        req_id: ReqId,
    },
    /// Host: end the current wait even though some participants are not ready.
    StartWithoutWaiting {
        req_id: ReqId,
    },
    /// Host: share one more folder of the server's machine (`path` as seen by the server).
    /// Without a name, the folder's own name is used.
    AddMediaFolder {
        req_id: ReqId,
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    /// Host: stop sharing a folder.
    RemoveMediaFolder {
        req_id: ReqId,
        name: String,
    },
}

impl ClientMsg {
    /// The request id of a mutating request, if this message carries one.
    pub fn req_id(&self) -> Option<ReqId> {
        use ClientMsg::*;
        match self {
            Play { req_id, .. }
            | Pause { req_id, .. }
            | Seek { req_id, .. }
            | SetSpeed { req_id, .. }
            | PlaylistAdd { req_id, .. }
            | PlaylistRemove { req_id, .. }
            | PlaylistMove { req_id, .. }
            | PlaylistSelect { req_id, .. }
            | SetControlMode { req_id, .. }
            | SetLagPolicy { req_id, .. }
            | SetReadyCheck { req_id, .. }
            | SetCanControl { req_id, .. }
            | Kick { req_id, .. }
            | RotateInvite { req_id }
            | RescanLibrary { req_id }
            | StartWithoutWaiting { req_id }
            | AddMediaFolder { req_id, .. }
            | RemoveMediaFolder { req_id, .. } => Some(*req_id),
            Hello { .. }
            | Ping { .. }
            | Bye
            | SetStatus { .. }
            | Progress { .. }
            | ItemEnded { .. }
            | Chat { .. }
            | SetReady { .. }
            | GetLibrary => None,
        }
    }
}

/// Error codes (spec §5.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum ErrorCode {
    BadToken,
    SessionExpired,
    VersionMismatch {
        server_protocol: u32,
    },
    NicknameTaken,
    RoomFull,
    NotPermitted,
    NotHost,
    UnknownItem,
    UnknownFile,
    InvalidRequest,
    /// Adding these items would make the playlist longer than the server allows.
    PlaylistFull {
        max_items: u32,
    },
    /// A media folder change was refused.
    MediaFolder {
        problem: MediaFolderProblem,
    },
}

/// Why a media folder cannot be added or removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MediaFolderProblem {
    /// The server does not allow changing folders remotely.
    ChangesDisabled,
    /// The path must be absolute on the server's machine.
    NotAbsolute,
    NotFound,
    NotAFolder,
    AlreadyShared,
    NameTaken,
    /// No shared folder with that name.
    UnknownFolder,
    /// The server could not save its config file.
    CannotSave,
    /// The folder is outside the server's `media_root`.
    OutsideMediaRoot,
    /// The server has no `media_root`, so folders cannot be added from the app.
    NoMediaRoot,
}

impl From<MediaFolderProblem> for ErrorCode {
    fn from(problem: MediaFolderProblem) -> Self {
        ErrorCode::MediaFolder { problem }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CloseReason {
    Kicked,
    ServerShutdown,
    ProtocolError,
}

/// Successful handshake reply.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Welcome {
    pub participant_id: ParticipantId,
    pub session_token: String,
    pub is_host: bool,
    pub server_time_ms: u64,
    pub room: RoomState,
    pub playback: PlaybackState,
    pub chat_history: Vec<ChatMessage>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum Event {
    Chat(ChatMessage),
    Notice {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<ParticipantId>,
        action: NoticeAction,
    },
}

/// Server → client messages (spec §5.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ServerMsg {
    Welcome(Welcome),
    Reject {
        code: ErrorCode,
        message: String,
    },
    Pong {
        client_time_ms: u64,
        server_time_ms: u64,
    },
    Playback(PlaybackState),
    Room(RoomState),
    ProgressBatch {
        entries: Vec<ProgressEntry>,
    },
    Event(Event),
    Denied {
        req_id: ReqId,
        code: ErrorCode,
    },
    Library {
        roots: Vec<LibraryNode>,
    },
    InviteToken {
        token: String,
    },
    /// To hosts: the server's shared folders (on join and after every change).
    MediaFolders {
        folders: Vec<MediaFolder>,
        editable: bool,
        adding: FolderAdding,
    },
    Closing {
        reason: CloseReason,
    },
}

/// Parse one client text frame.
pub fn decode_client(text: &str) -> Result<ClientMsg, serde_json::Error> {
    serde_json::from_str(text)
}

/// Parse one server text frame.
pub fn decode_server(text: &str) -> Result<ServerMsg, serde_json::Error> {
    serde_json::from_str(text)
}

/// Serialize a message into one text frame.
pub fn encode<T: Serialize>(msg: &T) -> String {
    serde_json::to_string(msg).expect("protocol messages always serialize")
}
