//! Control rights, control modes and host-only administration (spec §4.4).

use sppy_proto::{CloseReason, ControlMode, ErrorCode, NoticeAction, ParticipantId, ReqId};

use super::{Effect, Room};

impl Room {
    /// Whether `pid` may control playback and edit the playlist.
    pub fn can_control(&self, pid: ParticipantId) -> bool {
        let Some(m) = self.member(pid) else {
            return false;
        };
        m.info.is_host
            || match self.control_mode {
                ControlMode::Everyone => true,
                ControlMode::HostOnly => false,
                ControlMode::Selected => m.info.can_control,
            }
    }

    pub(super) fn set_control_mode(
        &mut self,
        pid: ParticipantId,
        req_id: ReqId,
        mode: ControlMode,
    ) -> Vec<Effect> {
        if !self.is_host(pid) {
            return self.denied(pid, req_id, ErrorCode::NotHost);
        }
        self.control_mode = mode;
        vec![
            self.broadcast_room(),
            Self::notice(Some(pid), NoticeAction::ControlModeChanged { mode }),
            Effect::Persist,
        ]
    }

    pub(super) fn set_can_control(
        &mut self,
        pid: ParticipantId,
        req_id: ReqId,
        target: ParticipantId,
        allowed: bool,
    ) -> Vec<Effect> {
        if !self.is_host(pid) {
            return self.denied(pid, req_id, ErrorCode::NotHost);
        }
        let Some(m) = self.member_mut(target) else {
            return self.denied(pid, req_id, ErrorCode::InvalidRequest);
        };
        m.info.can_control = allowed;
        vec![
            self.broadcast_room(),
            Self::notice(
                Some(pid),
                NoticeAction::CanControlChanged {
                    participant_id: target,
                    allowed,
                },
            ),
        ]
    }

    pub(super) fn kick(
        &mut self,
        pid: ParticipantId,
        req_id: ReqId,
        target: ParticipantId,
    ) -> Vec<Effect> {
        if !self.is_host(pid) {
            return self.denied(pid, req_id, ErrorCode::NotHost);
        }
        if target == pid || !self.remove_member(target) {
            return self.denied(pid, req_id, ErrorCode::InvalidRequest);
        }
        vec![
            Effect::Close {
                to: target,
                reason: CloseReason::Kicked,
            },
            self.broadcast_room(),
            Self::notice(
                Some(pid),
                NoticeAction::Kicked {
                    participant_id: target,
                },
            ),
        ]
    }
}
