//! Participant statuses, holds, end of item, delivery progress and chat (spec §4.5, §6.4).

use sppy_proto::{
    ChatMessage, Delivery, ErrorCode, Event, Hold, HoldReason, ItemId, MediaStatus, NoticeAction,
    ParticipantId, ProgressEntry, ReqId, ServerMsg, SkipReason, Source,
};

use super::{Effect, Room};

const CHAT_HISTORY: usize = 50;
const MAX_CHAT_CHARS: usize = 500;
const PROGRESS_INTERVAL_MS: u64 = 1000;
/// `ItemEnded` is accepted only this close to the known duration.
/// Without a duration of its own, the server moves on this long after the largest
/// reported end even if not every player has reported reaching it.
const LATE_END_MS: u64 = 10_000;
/// With an unknown duration, `ItemEnded` before this position is a stale report.
const MIN_PLAYED_MS: u64 = 3000;

impl Room {
    pub(super) fn set_status(
        &mut self,
        now_ms: u64,
        pid: ParticipantId,
        item_id: ItemId,
        status: MediaStatus,
    ) -> Vec<Effect> {
        if self.playback.item_id != Some(item_id) {
            return Vec::new();
        }
        let Some(m) = self.member_mut(pid) else {
            return Vec::new();
        };
        m.info.status = status.clone();
        match status {
            MediaStatus::Stalled => {
                self.stalled_since.entry(pid).or_insert(now_ms);
            }
            _ => {
                self.stalled_since.remove(&pid);
            }
        }
        let server_knows = self.playlist.iter().find(|i| i.item_id == item_id).is_some_and(|i| {
            matches!(&i.source, Source::File { file_id }
                if self.catalog.get(file_id).is_some_and(|f| f.duration_ms.is_some()))
        });
        if let MediaStatus::Ready { duration_ms, title } = &status
            && let Some(item) = self.playlist.iter_mut().find(|i| i.item_id == item_id)
        {
            // Players' reports only when the server could not read the file; the largest
            // one, so that one participant cannot shorten the item for everyone.
            if !server_knows && duration_ms.is_some() && *duration_ms > item.duration_ms {
                item.duration_ms = *duration_ms;
            }
            if let (Source::Url { url }, Some(title)) = (&item.source, title)
                && item.title == *url
            {
                item.title = title.clone();
            }
        }
        if matches!(status, MediaStatus::Failed { .. }) && self.all_failed_on_url() {
            let next = self.item_after(item_id);
            let mut effects = vec![Self::notice(
                None,
                NoticeAction::ItemSkipped { item_id, reason: SkipReason::AllFailed },
            )];
            effects.extend(self.change_item(now_ms, next));
            effects.push(Effect::Persist);
            return effects;
        }
        let mut effects = vec![self.broadcast_room()];
        effects.extend(self.reevaluate_hold(now_ms));
        effects
    }

    /// Release the hold once everyone it waits for is ready or inactive.
    pub(super) fn reevaluate_hold(&mut self, now_ms: u64) -> Vec<Effect> {
        let Some(hold) = &self.playback.hold else {
            return Vec::new();
        };
        let still_waiting = hold.waiting_for.iter().any(|pid| {
            self.member(*pid).is_some_and(|m| {
                self.is_active(m.info.id) && !matches!(m.info.status, MediaStatus::Ready { .. })
            })
        });
        if still_waiting {
            return Vec::new();
        }
        self.playback.hold = None;
        self.playback.at_server_ms = now_ms;
        self.playback_changed();
        self.playback.cause = None;
        vec![self.broadcast_playback(), Self::notice(None, NoticeAction::HoldEnded)]
    }

    /// Host override: end the current hold even though someone is not ready.
    pub(super) fn start_without_waiting(
        &mut self,
        now_ms: u64,
        pid: ParticipantId,
        req_id: ReqId,
    ) -> Vec<Effect> {
        if !self.is_host(pid) {
            return self.denied(pid, req_id, ErrorCode::NotHost);
        }
        if self.playback.hold.take().is_none() {
            return Vec::new();
        }
        self.playback.at_server_ms = now_ms;
        self.playback_changed();
        self.playback.cause = Some(sppy_proto::Cause { participant_id: pid, req_id });
        vec![self.broadcast_playback(), Self::notice(Some(pid), NoticeAction::HoldEnded)]
    }

    /// Start a `Stalled` hold for participants stalled for at least `stall_hold_secs`.
    pub(super) fn check_stalls(&mut self, now_ms: u64) -> Vec<Effect> {
        let limit_ms = u64::from(self.config.settings.stall_hold_secs) * 1000;
        if limit_ms == 0 || !self.playback.is_playing() {
            return Vec::new();
        }
        let mut stalled: Vec<ParticipantId> = self
            .stalled_since
            .iter()
            .filter(|(pid, since)| {
                now_ms.saturating_sub(**since) >= limit_ms && self.is_active(**pid)
            })
            .map(|(pid, _)| *pid)
            .collect();
        if stalled.is_empty() {
            return Vec::new();
        }
        stalled.sort();
        let position = self.playback.expected_position_ms(now_ms);
        self.rebase(now_ms, position);
        let hold = Hold { reason: HoldReason::Stalled, waiting_for: stalled };
        self.playback.hold = Some(hold.clone());
        self.playback_changed();
        self.playback.cause = None;
        vec![self.broadcast_playback(), Self::notice(None, NoticeAction::HoldStarted { hold })]
    }

    /// A player reached the end. Counts only when the server cannot tell the end itself (no
    /// duration of its own): then the item ends once every active participant got there.
    pub(super) fn item_ended(
        &mut self,
        now_ms: u64,
        pid: ParticipantId,
        item_id: ItemId,
    ) -> Vec<Effect> {
        if self.playback.item_id != Some(item_id) || self.server_duration().is_some() {
            return Vec::new();
        }
        // A stale EOF right after an item change is not an end: at least 3 s, or half the
        // reported length for shorter clips, must have played.
        let reported = self.current_item().and_then(|i| i.duration_ms);
        let min_played = reported.map_or(MIN_PLAYED_MS, |d| MIN_PLAYED_MS.min(d / 2));
        if self.playback.hold.is_some() || self.playback.expected_position_ms(now_ms) < min_played {
            return Vec::new();
        }
        self.ended.insert(pid);
        self.check_item_end(now_ms)
    }

    /// Called every tick and on every end report: move on when the item is over.
    pub(super) fn check_item_end(&mut self, now_ms: u64) -> Vec<Effect> {
        let Some(item) = self.current_item() else { return Vec::new() };
        if self.playback.hold.is_some() {
            return Vec::new();
        }
        let expected = self.playback.expected_position_ms(now_ms);
        let over = match self.server_duration() {
            Some(d) => !self.playback.paused && expected >= d,
            None => {
                let active = self.active_ids();
                let everyone =
                    !self.ended.is_empty() && active.iter().all(|p| self.ended.contains(p));
                // A player that never reports its end does not hold the room forever.
                let late = item.duration_ms.is_some_and(|d| {
                    !self.playback.paused && expected >= d.saturating_add(LATE_END_MS)
                });
                everyone || late
            }
        };
        if over { self.finish_item(now_ms) } else { Vec::new() }
    }

    /// The next item, or a pause at the end of the last one.
    fn finish_item(&mut self, now_ms: u64) -> Vec<Effect> {
        let Some(item_id) = self.playback.item_id else { return Vec::new() };
        let duration = self.current_item().and_then(|i| i.duration_ms);
        let mut effects = match self.item_after(item_id) {
            Some(next) => self.change_item(now_ms, Some(next)),
            None => {
                if self.playback.paused {
                    return Vec::new();
                }
                let expected = self.playback.expected_position_ms(now_ms);
                self.rebase(now_ms, duration.map_or(expected, |d| d.min(expected)));
                self.playback.paused = true;
                self.playback_changed();
                self.playback.cause = None;
                vec![self.broadcast_playback()]
            }
        };
        effects.push(Effect::Persist);
        effects
    }

    pub(super) fn progress(
        &mut self,
        pid: ParticipantId,
        item_id: ItemId,
        delivery: Delivery,
        drift_ms: Option<i64>,
    ) -> Vec<Effect> {
        if self.playback.item_id != Some(item_id) || self.member(pid).is_none() {
            return Vec::new();
        }
        let fell_back = matches!(
            self.progress.get(&pid),
            Some(ProgressEntry { delivery: Delivery::Download { .. }, item_id: prev, .. }) if prev == &item_id
        ) && delivery == Delivery::Stream;
        self.progress
            .insert(pid, ProgressEntry { participant_id: pid, item_id, delivery, drift_ms });
        self.progress_dirty = true;
        if fell_back {
            vec![Self::notice(Some(pid), NoticeAction::DeliveryFellBack { participant_id: pid })]
        } else {
            Vec::new()
        }
    }

    pub(super) fn flush_progress(&mut self, now_ms: u64) -> Vec<Effect> {
        if !self.progress_dirty
            || self
                .last_progress_ms
                .is_some_and(|t| now_ms.saturating_sub(t) < PROGRESS_INTERVAL_MS)
        {
            return Vec::new();
        }
        self.progress_dirty = false;
        self.last_progress_ms = Some(now_ms);
        let mut entries: Vec<ProgressEntry> = self.progress.values().cloned().collect();
        entries.sort_by_key(|e| e.participant_id);
        vec![Effect::Broadcast { msg: ServerMsg::ProgressBatch { entries } }]
    }

    pub(super) fn chat(&mut self, now_ms: u64, pid: ParticipantId, text: String) -> Vec<Effect> {
        if text.trim().is_empty() || text.chars().count() > MAX_CHAT_CHARS {
            return Vec::new();
        }
        let Some(m) = self.members.iter_mut().find(|m| m.info.id == pid) else {
            return Vec::new();
        };
        if !m.chat_rate.try_take(now_ms) {
            return Vec::new();
        }
        let msg = ChatMessage {
            from: pid,
            nickname: m.info.nickname.clone(),
            text,
            at_server_ms: now_ms,
        };
        self.chat.push_back(msg.clone());
        while self.chat.len() > CHAT_HISTORY {
            self.chat.pop_front();
        }
        vec![Effect::Broadcast { msg: ServerMsg::Event(Event::Chat(msg)) }]
    }

    /// Every active participant failed on the current URL item.
    fn all_failed_on_url(&self) -> bool {
        if !matches!(self.current_item().map(|i| &i.source), Some(Source::Url { .. })) {
            return false;
        }
        let mut connected = self
            .members
            .iter()
            .filter(|m| m.info.connected && m.info.status != MediaStatus::PlayerClosed)
            .peekable();
        connected.peek().is_some()
            && connected.all(|m| matches!(m.info.status, MediaStatus::Failed { .. }))
    }
}
