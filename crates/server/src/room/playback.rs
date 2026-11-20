//! Playback commands: play, pause, seek, speed (spec §6.1).

use sppy_proto::{
    Cause, ErrorCode, FOLLOW_SLOWEST_MS, LagPolicy, MediaStatus, NoticeAction, ParticipantId,
    ReqId, ServerMsg,
};

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
                NoticeAction::Seeked { to_ms: self.playback.position_ms }
            }
            PlaybackCmd::SetSpeed(speed) => {
                self.rebase(now_ms, current);
                self.playback.speed = speed;
                NoticeAction::SpeedChanged { speed }
            }
        };
        self.playback_changed();
        self.playback.cause = Some(Cause { participant_id: pid, req_id });
        vec![self.broadcast_playback(), Self::notice(Some(pid), action), Effect::Persist]
    }

    /// With [`LagPolicy::FollowSlowest`]: go back to the participant furthest behind, if
    /// they are more than [`FOLLOW_SLOWEST_MS`] behind. Only participants who are ready
    /// and measured their drift count; buffering ones are the stall hold's business.
    pub(super) fn follow_slowest(&mut self, now_ms: u64) -> Vec<Effect> {
        if self.lag_policy != LagPolicy::FollowSlowest
            || !self.playback.is_playing()
            || self.playback.item_id.is_none()
        {
            return Vec::new();
        }
        let slowest = self
            .progress
            .values()
            .filter(|e| Some(e.item_id) == self.playback.item_id)
            .filter(|e| {
                self.is_active(e.participant_id)
                    && self
                        .member(e.participant_id)
                        .is_some_and(|m| matches!(m.info.status, MediaStatus::Ready { .. }))
            })
            .filter_map(|e| e.drift_ms.map(|d| (e.participant_id, d)))
            .min_by_key(|&(_, d)| d);
        let Some((pid, drift)) = slowest else { return Vec::new() };
        let back_ms = drift.unsigned_abs();
        if drift >= 0 || back_ms <= FOLLOW_SLOWEST_MS {
            return Vec::new();
        }
        let position = self.playback.expected_position_ms(now_ms).saturating_sub(back_ms);
        self.rebase(now_ms, position);
        // Forgets the drifts measured against the old position.
        self.playback_changed();
        self.playback.cause = None;
        vec![
            self.broadcast_playback(),
            Self::notice(None, NoticeAction::WentBackFor { participant_id: pid, back_ms }),
        ]
    }

    /// Set `P` at `T = now`, clamped to the current item's known duration.
    pub(super) fn rebase(&mut self, now_ms: u64, position_ms: u64) {
        let limit = self.current_item().and_then(|i| i.duration_ms).unwrap_or(u64::MAX);
        self.playback.position_ms = position_ms.min(limit);
        self.playback.at_server_ms = now_ms;
    }

    pub(super) fn broadcast_playback(&self) -> Effect {
        Effect::Broadcast { msg: ServerMsg::Playback(self.playback.clone()) }
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
