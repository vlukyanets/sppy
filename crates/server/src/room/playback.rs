//! Playback commands: play, pause, seek, speed (spec §6.1).

use sppy_proto::{Cause, ErrorCode, NoticeAction, ParticipantId, ReqId, ServerMsg};

use super::{Effect, Room};

const MIN_SPEED: f64 = 0.25;
const MAX_SPEED: f64 = 4.0;

pub(super) enum PlaybackCmd {
    Play(Option<u64>),
    Pause(Option<u64>),
    Seek(u64),
    SetSpeed(f64),
}

impl Room {
    pub(super) fn playback_command(
        &mut self,
        now_ms: u64,
        pid: ParticipantId,
        req_id: ReqId,
        cmd: PlaybackCmd,
    ) -> Vec<Effect> {
        if !self.can_control(pid) {
            return self.denied(pid, req_id, ErrorCode::NotPermitted);
        }
        if self.playback.item_id.is_none() {
            return self.denied(pid, req_id, ErrorCode::InvalidRequest);
        }
        if let PlaybackCmd::SetSpeed(speed) = cmd
            && !(MIN_SPEED..=MAX_SPEED).contains(&speed)
        {
            return self.denied(pid, req_id, ErrorCode::InvalidRequest);
        }
        let current = self.playback.expected_position_ms(now_ms);
        let action = match cmd {
            PlaybackCmd::Play(pos) => {
                self.rebase(now_ms, pos.unwrap_or(current));
                self.playback.paused = false;
                NoticeAction::Resumed
            }
            PlaybackCmd::Pause(pos) => {
                self.rebase(now_ms, pos.unwrap_or(current));
                self.playback.paused = true;
                NoticeAction::Paused
            }
            PlaybackCmd::Seek(pos) => {
                self.rebase(now_ms, pos);
                NoticeAction::Seeked {
                    to_ms: self.playback.position_ms,
                }
            }
            PlaybackCmd::SetSpeed(speed) => {
                self.rebase(now_ms, current);
                self.playback.speed = speed;
                NoticeAction::SpeedChanged { speed }
            }
        };
        self.playback.seq += 1;
        self.playback.cause = Some(Cause {
            participant_id: pid,
            req_id,
        });
        vec![
            self.broadcast_playback(),
            Self::notice(Some(pid), action),
            Effect::Persist,
        ]
    }

    /// Set `P` at `T = now`, clamped to the current item's known duration.
    pub(super) fn rebase(&mut self, now_ms: u64, position_ms: u64) {
        let limit = self
            .current_item()
            .and_then(|i| i.duration_ms)
            .unwrap_or(u64::MAX);
        self.playback.position_ms = position_ms.min(limit);
        self.playback.at_server_ms = now_ms;
    }

    pub(super) fn broadcast_playback(&self) -> Effect {
        Effect::Broadcast {
            msg: ServerMsg::Playback(self.playback.clone()),
        }
    }

    #[cfg(test)]
    pub(crate) fn test_set_current(&mut self, item: sppy_proto::Item) {
        self.playback.item_id = Some(item.item_id);
        self.playlist.push(item);
    }

    #[cfg(test)]
    pub(crate) fn test_set_hold(&mut self, hold: Option<sppy_proto::Hold>) {
        self.playback.hold = hold;
    }
}
