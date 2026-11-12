//! Pure room state machine (spec §4.9). No I/O: time is passed in, effects are returned.

mod participants;
mod permissions;
mod playback;

use std::collections::VecDeque;

use sppy_proto::{
    ChatMessage, ClientMsg, CloseReason, ControlMode, ErrorCode, Event, Item, NoticeAction,
    Participant, ParticipantId, PlaybackState, ReqId, RoomId, RoomSettings, RoomState, ServerMsg,
    Welcome,
};

pub use participants::{JoinRequest, JoinRole, Joined};
use playback::PlaybackCmd;

/// Static room configuration, derived from the server config.
#[derive(Debug, Clone)]
pub struct RoomConfig {
    pub max_participants: usize,
    pub resume_grace_ms: u64,
    pub settings: RoomSettings,
}

/// Something the I/O layer must do after the room handled an input.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    Send {
        to: ParticipantId,
        msg: ServerMsg,
    },
    Broadcast {
        msg: ServerMsg,
    },
    Close {
        to: ParticipantId,
        reason: CloseReason,
    },
    Persist,
}

#[derive(Debug, Clone)]
struct Member {
    info: Participant,
    session_token: String,
    disconnected_at: Option<u64>,
}

/// The whole pure state of the single room.
#[derive(Debug, Clone)]
pub struct Room {
    config: RoomConfig,
    members: Vec<Member>,
    next_participant_id: u64,
    control_mode: ControlMode,
    playlist: Vec<Item>,
    playback: PlaybackState,
    chat: VecDeque<ChatMessage>,
}

impl Room {
    pub fn new(config: RoomConfig) -> Room {
        Room {
            config,
            members: Vec::new(),
            next_participant_id: 1,
            control_mode: ControlMode::Everyone,
            playlist: Vec::new(),
            playback: PlaybackState {
                item_id: None,
                position_ms: 0,
                at_server_ms: 0,
                paused: true,
                speed: 1.0,
                hold: None,
                seq: 0,
                cause: None,
            },
            chat: VecDeque::new(),
        }
    }

    /// Snapshot of the room as sent to clients.
    pub fn state(&self) -> RoomState {
        RoomState {
            room_id: RoomId::main(),
            participants: self.members.iter().map(|m| m.info.clone()).collect(),
            control_mode: self.control_mode,
            playlist: self.playlist.clone(),
            settings: self.config.settings,
        }
    }

    pub fn playback(&self) -> &PlaybackState {
        &self.playback
    }

    pub fn is_host(&self, pid: ParticipantId) -> bool {
        self.member(pid).is_some_and(|m| m.info.is_host)
    }

    /// Whether a session token belongs to a current (possibly disconnected) participant.
    pub fn session_valid(&self, session_token: &str) -> bool {
        self.members
            .iter()
            .any(|m| m.session_token == session_token)
    }

    /// Periodic housekeeping (every 250 ms from the I/O layer).
    pub fn tick(&mut self, now_ms: u64) -> Vec<Effect> {
        self.expire_disconnected(now_ms)
    }

    /// Handle a room-level client request from `pid`.
    pub fn command(&mut self, now_ms: u64, pid: ParticipantId, msg: ClientMsg) -> Vec<Effect> {
        match msg {
            ClientMsg::Play {
                req_id,
                position_ms,
            } => self.playback_command(now_ms, pid, req_id, PlaybackCmd::Play(position_ms)),
            ClientMsg::Pause {
                req_id,
                position_ms,
            } => self.playback_command(now_ms, pid, req_id, PlaybackCmd::Pause(position_ms)),
            ClientMsg::Seek {
                req_id,
                position_ms,
            } => self.playback_command(now_ms, pid, req_id, PlaybackCmd::Seek(position_ms)),
            ClientMsg::SetSpeed { req_id, speed } => {
                self.playback_command(now_ms, pid, req_id, PlaybackCmd::SetSpeed(speed))
            }
            ClientMsg::SetControlMode { req_id, mode } => self.set_control_mode(pid, req_id, mode),
            ClientMsg::SetCanControl {
                req_id,
                participant_id,
                allowed,
            } => self.set_can_control(pid, req_id, participant_id, allowed),
            ClientMsg::Kick {
                req_id,
                participant_id,
            } => self.kick(pid, req_id, participant_id),
            other => match other.req_id() {
                Some(req_id) => self.denied(pid, req_id, ErrorCode::InvalidRequest),
                None => Vec::new(),
            },
        }
    }

    /// `Denied` followed by a resync of playback and room, to `pid` only.
    fn denied(&self, pid: ParticipantId, req_id: ReqId, code: ErrorCode) -> Vec<Effect> {
        vec![
            Effect::Send {
                to: pid,
                msg: ServerMsg::Denied { req_id, code },
            },
            Effect::Send {
                to: pid,
                msg: ServerMsg::Playback(self.playback.clone()),
            },
            Effect::Send {
                to: pid,
                msg: ServerMsg::Room(self.state()),
            },
        ]
    }

    fn current_item(&self) -> Option<&Item> {
        let id = self.playback.item_id?;
        self.playlist.iter().find(|i| i.item_id == id)
    }

    fn member(&self, pid: ParticipantId) -> Option<&Member> {
        self.members.iter().find(|m| m.info.id == pid)
    }

    fn member_mut(&mut self, pid: ParticipantId) -> Option<&mut Member> {
        self.members.iter_mut().find(|m| m.info.id == pid)
    }

    fn welcome(&self, now_ms: u64, pid: ParticipantId) -> Effect {
        let m = self.member(pid).expect("welcome for existing member");
        Effect::Send {
            to: pid,
            msg: ServerMsg::Welcome(Welcome {
                participant_id: pid,
                session_token: m.session_token.clone(),
                is_host: m.info.is_host,
                server_time_ms: now_ms,
                room: self.state(),
                playback: self.playback.clone(),
                chat_history: self.chat.iter().cloned().collect(),
            }),
        }
    }

    fn broadcast_room(&self) -> Effect {
        Effect::Broadcast {
            msg: ServerMsg::Room(self.state()),
        }
    }

    fn notice(actor: Option<ParticipantId>, action: NoticeAction) -> Effect {
        Effect::Broadcast {
            msg: ServerMsg::Event(Event::Notice { actor, action }),
        }
    }

    #[cfg(test)]
    pub(crate) fn test_set_can_control(&mut self, pid: ParticipantId, allowed: bool) {
        self.member_mut(pid).unwrap().info.can_control = allowed;
    }
}

#[cfg(test)]
mod tests;
