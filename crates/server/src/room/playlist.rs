//! Playlist editing, item changes and the library catalog (spec §4.5).

use sppy_proto::{
    ErrorCode, FileId, Fingerprint, Hold, HoldReason, Item, ItemId, MediaStatus, NoticeAction,
    ParticipantId, Repeat, ReqId, SkipReason, Source, Subtitle,
};

/// Fisher–Yates with a small generator (splitmix64): enough for a playlist, no dependency.
fn shuffle<T>(items: &mut [T], seed: u64) {
    let mut state = seed;
    let mut next = || {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    for i in (1..items.len()).rev() {
        let j = (next() % (i as u64 + 1)) as usize;
        items.swap(i, j);
    }
}

use super::{Effect, MAX_PLAYLIST_ITEMS, MAX_URL_CHARS, Room};

/// What the room needs to know about a library file.
#[derive(Debug, Clone, PartialEq)]
pub struct FileInfo {
    pub file_id: FileId,
    pub title: String,
    pub fingerprint: Option<Fingerprint>,
    /// From the file's header: the server's own knowledge, above what players report.
    pub duration_ms: Option<u64>,
    pub subtitles: Vec<Subtitle>,
}

impl Room {
    /// Replace the catalog of known library files; refresh metadata of playlist items.
    pub fn set_library(&mut self, _now_ms: u64, files: Vec<FileInfo>) -> Vec<Effect> {
        self.catalog = files.into_iter().map(|f| (f.file_id.clone(), f)).collect();
        let mut changed = false;
        for item in &mut self.playlist {
            let Source::File { file_id } = &item.source else {
                continue;
            };
            let Some(info) = self.catalog.get(file_id) else {
                continue;
            };
            if item.fingerprint != info.fingerprint || item.subtitles != info.subtitles {
                item.fingerprint = info.fingerprint.clone();
                item.subtitles = info.subtitles.clone();
                changed = true;
            }
            // The server's own reading of the file wins over what players reported.
            if info.duration_ms.is_some() && item.duration_ms != info.duration_ms {
                item.duration_ms = info.duration_ms;
                changed = true;
            }
        }
        if changed { vec![self.broadcast_room()] } else { Vec::new() }
    }

    pub(super) fn playlist_add(
        &mut self,
        now_ms: u64,
        pid: ParticipantId,
        req_id: ReqId,
        sources: Vec<Source>,
        after: Option<ItemId>,
    ) -> Vec<Effect> {
        if !self.can_control(pid) {
            return self.denied(pid, req_id, ErrorCode::NotPermitted);
        }
        let too_long = |s: &Source| matches!(s, Source::Url { url } if url.len() > MAX_URL_CHARS);
        if sources.is_empty() || sources.iter().any(too_long) {
            return self.denied(pid, req_id, ErrorCode::InvalidRequest);
        }
        if self.playlist.len().saturating_add(sources.len()) > MAX_PLAYLIST_ITEMS {
            let max_items = u32::try_from(MAX_PLAYLIST_ITEMS).unwrap_or(u32::MAX);
            return self.denied(pid, req_id, ErrorCode::PlaylistFull { max_items });
        }
        let mut new_items = Vec::with_capacity(sources.len());
        for source in sources {
            let (title, fingerprint, subtitles, duration_ms) = match &source {
                Source::File { file_id } => match self.catalog.get(file_id) {
                    Some(info) => (
                        info.title.clone(),
                        info.fingerprint.clone(),
                        info.subtitles.clone(),
                        info.duration_ms,
                    ),
                    None => return self.denied(pid, req_id, ErrorCode::UnknownFile),
                },
                Source::Url { url } => (url.clone(), None, Vec::new(), None),
            };
            new_items.push(Item {
                item_id: ItemId(0),
                source,
                title,
                duration_ms,
                fingerprint,
                subtitles,
                added_by: pid,
                watched: false,
            });
        }
        // Without a place, at the end: the client may not know the items it added a moment
        // ago yet, so naming the last one would put a quick second add in the wrong place.
        let index = match after {
            None => Some(self.playlist.len()),
            Some(a) => self.position_of(a).map(|p| p + 1),
        };
        let Some(mut index) = index else {
            return self.denied(pid, req_id, ErrorCode::UnknownItem);
        };
        let mut ids = Vec::with_capacity(new_items.len());
        for mut item in new_items {
            item.item_id = ItemId(self.next_item_id);
            self.next_item_id += 1;
            ids.push(item.item_id);
            self.playlist.insert(index, item);
            index += 1;
        }
        let mut effects = Vec::new();
        if self.playback.item_id.is_none() {
            effects.extend(self.change_item_paused(now_ms, Some(ids[0]), true));
        } else {
            effects.push(self.broadcast_room());
        }
        effects.push(Self::notice(Some(pid), NoticeAction::PlaylistAdded { item_ids: ids }));
        effects.push(Effect::Persist);
        effects
    }

    pub(super) fn playlist_remove(
        &mut self,
        now_ms: u64,
        pid: ParticipantId,
        req_id: ReqId,
        item_id: ItemId,
    ) -> Vec<Effect> {
        if !self.can_control(pid) {
            return self.denied(pid, req_id, ErrorCode::NotPermitted);
        }
        let Some(pos) = self.position_of(item_id) else {
            return self.denied(pid, req_id, ErrorCode::UnknownItem);
        };
        let next = self.item_after(item_id);
        self.playlist.remove(pos);
        let mut effects = if self.playback.item_id == Some(item_id) {
            self.change_item(now_ms, next)
        } else {
            vec![self.broadcast_room()]
        };
        effects.push(Self::notice(Some(pid), NoticeAction::PlaylistRemoved { item_id }));
        effects.push(Effect::Persist);
        effects
    }

    pub(super) fn playlist_move(
        &mut self,
        pid: ParticipantId,
        req_id: ReqId,
        item_id: ItemId,
        after: Option<ItemId>,
    ) -> Vec<Effect> {
        if !self.can_control(pid) {
            return self.denied(pid, req_id, ErrorCode::NotPermitted);
        }
        if after == Some(item_id) {
            return self.denied(pid, req_id, ErrorCode::InvalidRequest);
        }
        let Some(pos) = self.position_of(item_id) else {
            return self.denied(pid, req_id, ErrorCode::UnknownItem);
        };
        if after.is_some_and(|a| self.position_of(a).is_none()) {
            return self.denied(pid, req_id, ErrorCode::UnknownItem);
        }
        let item = self.playlist.remove(pos);
        let index = self.insert_index(after).expect("checked above");
        self.playlist.insert(index, item);
        vec![
            self.broadcast_room(),
            Self::notice(Some(pid), NoticeAction::PlaylistMoved { item_id }),
            Effect::Persist,
        ]
    }

    pub(super) fn playlist_select(
        &mut self,
        now_ms: u64,
        pid: ParticipantId,
        req_id: ReqId,
        item_id: ItemId,
    ) -> Vec<Effect> {
        if !self.can_control(pid) {
            return self.denied(pid, req_id, ErrorCode::NotPermitted);
        }
        if self.position_of(item_id).is_none() {
            return self.denied(pid, req_id, ErrorCode::UnknownItem);
        }
        let mut effects = self.change_item(now_ms, Some(item_id));
        effects.push(Effect::Persist);
        effects
    }

    /// Put the items after the current one (all of them without one) in a random order.
    /// The room stays pure: the order comes from the time of the request.
    pub(super) fn playlist_shuffle(
        &mut self,
        now_ms: u64,
        pid: ParticipantId,
        req_id: ReqId,
    ) -> Vec<Effect> {
        if !self.can_control(pid) {
            return self.denied(pid, req_id, ErrorCode::NotPermitted);
        }
        let start = self.playback.item_id.and_then(|id| self.position_of(id)).map_or(0, |p| p + 1);
        shuffle(&mut self.playlist[start..], now_ms ^ self.next_item_id.rotate_left(32));
        vec![
            self.broadcast_room(),
            Self::notice(Some(pid), NoticeAction::PlaylistShuffled),
            Effect::Persist,
        ]
    }

    pub(super) fn set_watched(
        &mut self,
        pid: ParticipantId,
        req_id: ReqId,
        item_id: ItemId,
        watched: bool,
    ) -> Vec<Effect> {
        if !self.can_control(pid) {
            return self.denied(pid, req_id, ErrorCode::NotPermitted);
        }
        let Some(item) = self.playlist.iter_mut().find(|i| i.item_id == item_id) else {
            return self.denied(pid, req_id, ErrorCode::UnknownItem);
        };
        item.watched = watched;
        vec![self.broadcast_room(), Effect::Persist]
    }

    pub(super) fn set_repeat(
        &mut self,
        pid: ParticipantId,
        req_id: ReqId,
        repeat: Repeat,
    ) -> Vec<Effect> {
        if !self.can_control(pid) {
            return self.denied(pid, req_id, ErrorCode::NotPermitted);
        }
        self.repeat = repeat;
        vec![
            self.broadcast_room(),
            Self::notice(Some(pid), NoticeAction::RepeatChanged { repeat }),
            Effect::Persist,
        ]
    }

    /// Make `item_id` current (skipping items whose file vanished), or clear the current item.
    pub(super) fn change_item(&mut self, now_ms: u64, item_id: Option<ItemId>) -> Vec<Effect> {
        self.change_item_paused(now_ms, item_id, false)
    }

    /// Like [`Self::change_item`]; `paused` sets the human pause flag of the new item.
    pub(super) fn change_item_paused(
        &mut self,
        now_ms: u64,
        item_id: Option<ItemId>,
        paused: bool,
    ) -> Vec<Effect> {
        let mut effects = Vec::new();
        let mut next = item_id;
        while let Some(id) = next {
            if self.file_missing(id) {
                effects.push(Self::notice(
                    None,
                    NoticeAction::ItemSkipped { item_id: id, reason: SkipReason::FileMissing },
                ));
                next = self.item_after(id);
            } else {
                break;
            }
        }
        // A closed player stays closed (and inactive) across item changes.
        for m in self.members.iter_mut() {
            m.info.ready = false;
            if m.info.status != MediaStatus::PlayerClosed {
                m.info.status = MediaStatus::Loading;
            }
        }
        self.on_item_changed();
        self.playback.item_id = next;
        self.playback.position_ms = 0;
        self.playback.at_server_ms = now_ms;
        self.playback_changed();
        self.playback.cause = None;
        match next {
            Some(_) => {
                self.playback.paused = paused;
                self.playback.hold =
                    Some(Hold { reason: HoldReason::ItemStart, waiting_for: self.active_ids() });
            }
            None => {
                self.playback.paused = true;
                self.playback.hold = None;
            }
        }
        // The state first: notices name items, which clients look up in it.
        let mut out = vec![self.broadcast_room(), self.broadcast_playback()];
        out.extend(effects);
        if let Some(id) = next {
            out.push(Self::notice(None, NoticeAction::ItemChanged { item_id: id }));
        }
        out
    }

    fn file_missing(&self, id: ItemId) -> bool {
        self.playlist.iter().find(|i| i.item_id == id).is_some_and(|i| match &i.source {
            Source::File { file_id } => !self.catalog.contains_key(file_id),
            Source::Url { .. } => false,
        })
    }

    pub(super) fn position_of(&self, id: ItemId) -> Option<usize> {
        self.playlist.iter().position(|i| i.item_id == id)
    }

    pub(super) fn item_after(&self, id: ItemId) -> Option<ItemId> {
        let pos = self.position_of(id)?;
        self.playlist.get(pos + 1).map(|i| i.item_id)
    }

    /// Index to insert at for `after` (None = start). None if `after` is unknown.
    fn insert_index(&self, after: Option<ItemId>) -> Option<usize> {
        match after {
            None => Some(0),
            Some(a) => self.position_of(a).map(|p| p + 1),
        }
    }
}
