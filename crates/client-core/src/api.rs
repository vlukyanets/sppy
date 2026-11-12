//! Public interface of the client core: commands in, snapshots and events out.

use std::collections::HashMap;
use std::path::PathBuf;

use sppy_player::PlayerLauncher;
use sppy_proto::{
    ChatMessage, ControlMode, ErrorCode, ItemId, LibraryNode, ParticipantId, PlaybackState,
    ProgressEntry, RoomState, Source,
};
use tokio::sync::{broadcast, mpsc, watch};
use tokio::time::Instant;

use crate::config::{ClientConfig, DeliveryMode};

#[derive(Debug, Clone, PartialEq)]
pub enum UiCommand {
    /// Join with an invite link (`sppy://host:port/<token>`).
    Connect {
        address: String,
        nickname: String,
    },
    /// Join the embedded server on localhost with the host token.
    BecomeHostConnect {
        port: u16,
        host_token: String,
    },
    Disconnect,
    Play,
    Pause,
    Seek(u64),
    SetSpeed(f64),
    PlaylistAdd(Vec<Source>),
    PlaylistRemove(ItemId),
    PlaylistMove {
        item_id: ItemId,
        after: Option<ItemId>,
    },
    PlaylistSelect(ItemId),
    Chat(String),
    GetLibrary,
    SetControlMode(ControlMode),
    SetCanControl(ParticipantId, bool),
    Kick(ParticipantId),
    RotateInvite,
    RescanLibrary,
    OpenPlayer,
    SetMode(DeliveryMode),
    ChooseLocalFile {
        item_id: ItemId,
        path: PathBuf,
    },
    ClearCache,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub enum ConnState {
    #[default]
    Idle,
    Connecting,
    Connected,
    Reconnecting {
        attempt: u32,
    },
    Rejected(ErrorCode),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct LocalStatus {
    pub mode: DeliveryMode,
    pub cache_used_bytes: u64,
}

/// Everything the UI needs to draw.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Snapshot {
    pub connection: ConnState,
    pub me: Option<ParticipantId>,
    pub is_host: bool,
    pub room: Option<RoomState>,
    pub playback: Option<PlaybackState>,
    pub progress: HashMap<ParticipantId, ProgressEntry>,
    pub library: Option<Vec<LibraryNode>>,
    pub invite_token: Option<String>,
    pub player_open: bool,
    /// The local player's position (extrapolated), once a file is loaded.
    pub player_position_ms: Option<u64>,
    pub chat_history: Vec<ChatMessage>,
    pub local: LocalStatus,
    /// `server_time − local_time` relative to [`ClientHandle::epoch`].
    pub server_offset_ms: Option<i64>,
}

impl Snapshot {
    /// Whether I may control playback and edit the playlist (spec §4.4).
    pub fn can_control(&self) -> bool {
        let (Some(me), Some(room)) = (self.me, &self.room) else { return false };
        let Some(p) = room.participants.iter().find(|p| p.id == me) else { return false };
        p.is_host
            || match room.control_mode {
                ControlMode::Everyone => true,
                ControlMode::HostOnly => false,
                ControlMode::Selected => p.can_control,
            }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum UiEvent {
    Chat(ChatMessage),
    Notice(String),
    Error(String),
}

pub struct ClientHandle {
    pub commands: mpsc::UnboundedSender<UiCommand>,
    pub state: watch::Receiver<Snapshot>,
    pub events: broadcast::Receiver<UiEvent>,
    events_tx: broadcast::Sender<UiEvent>,
    pub epoch: Instant,
}

impl ClientHandle {
    pub fn subscribe_events(&self) -> broadcast::Receiver<UiEvent> {
        self.events_tx.subscribe()
    }

    /// Where the room should be right now (for progress display and tests).
    pub fn expected_position_ms(&self) -> Option<u64> {
        let s = self.state.borrow();
        let offset = s.server_offset_ms?;
        let now_server = (self.epoch.elapsed().as_millis() as i64 + offset).max(0) as u64;
        Some(s.playback.as_ref()?.expected_position_ms(now_server))
    }
}

/// Start the client core. The player is launched once connected.
pub fn spawn_client<L: PlayerLauncher>(config: ClientConfig, launcher: L) -> ClientHandle {
    let (commands, cmd_rx) = mpsc::unbounded_channel();
    let mut initial = Snapshot::default();
    initial.local.mode = config.mode;
    let (state_tx, state) = watch::channel(initial);
    let (events_tx, events) = broadcast::channel(256);
    let epoch = Instant::now();
    let session =
        crate::session::Session::new(config, launcher, epoch, state_tx, events_tx.clone());
    tokio::spawn(session.run(cmd_rx));
    ClientHandle { commands, state, events, events_tx, epoch }
}

#[cfg(test)]
mod tests {
    use sppy_proto::*;

    use super::*;

    fn snap(mode: ControlMode, is_host: bool, can_control: bool) -> Snapshot {
        Snapshot {
            me: Some(ParticipantId(2)),
            room: Some(RoomState {
                room_id: RoomId::main(),
                participants: vec![Participant {
                    id: ParticipantId(2),
                    nickname: "me".into(),
                    is_host,
                    can_control,
                    connected: true,
                    status: MediaStatus::Loading,
                }],
                control_mode: mode,
                playlist: vec![],
                settings: RoomSettings { stall_hold_secs: 10, resume_buffer_secs: 10 },
            }),
            ..Snapshot::default()
        }
    }

    #[test]
    fn can_control_follows_spec_rules() {
        assert!(snap(ControlMode::Everyone, false, false).can_control());
        assert!(!snap(ControlMode::HostOnly, false, true).can_control());
        assert!(snap(ControlMode::HostOnly, true, false).can_control());
        assert!(!snap(ControlMode::Selected, false, false).can_control());
        assert!(snap(ControlMode::Selected, false, true).can_control());
        assert!(!Snapshot::default().can_control());
    }
}
