//! Human-readable text for server notices (also shown as OSD).

use sppy_proto::{ControlMode, ItemId, NoticeAction, ParticipantId, RoomState, SkipReason};

/// `h:mm:ss` or `m:ss`.
pub fn fmt_time(ms: u64) -> String {
    let s = ms / 1000;
    let (h, m, s) = (s / 3600, (s / 60) % 60, s % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

pub fn nickname(room: &RoomState, pid: ParticipantId) -> String {
    room.participants
        .iter()
        .find(|p| p.id == pid)
        .map(|p| p.nickname.clone())
        .unwrap_or_else(|| "someone".into())
}

fn title(room: &RoomState, id: ItemId) -> String {
    room.playlist
        .iter()
        .find(|i| i.item_id == id)
        .map(|i| i.title.clone())
        .unwrap_or_else(|| "item".into())
}

/// Whether a notice deserves the player's OSD: what others did to playback, item changes and
/// waits. Joins, playlist edits and settings only go to the app's event log.
pub fn osd_worthy(action: &NoticeAction, actor_is_me: bool) -> bool {
    match action {
        NoticeAction::Paused
        | NoticeAction::Resumed
        | NoticeAction::Seeked { .. }
        | NoticeAction::SpeedChanged { .. } => !actor_is_me,
        NoticeAction::ItemChanged { .. }
        | NoticeAction::HoldStarted { .. }
        | NoticeAction::HoldEnded
        | NoticeAction::ItemSkipped { .. }
        | NoticeAction::Kicked { .. } => true,
        NoticeAction::PlaylistAdded { .. }
        | NoticeAction::PlaylistRemoved { .. }
        | NoticeAction::PlaylistMoved { .. }
        | NoticeAction::Joined
        | NoticeAction::Left
        | NoticeAction::ControlModeChanged { .. }
        | NoticeAction::CanControlChanged { .. }
        | NoticeAction::DeliveryFellBack { .. } => false,
    }
}

/// "Waiting for: you (downloading 45%), yaroslav (buffering), ann (loading)".
pub fn waiting_line(
    hold: &sppy_proto::Hold,
    room: &RoomState,
    progress: &std::collections::HashMap<ParticipantId, sppy_proto::ProgressEntry>,
    me: Option<ParticipantId>,
) -> String {
    use sppy_proto::{Delivery, MediaStatus};
    let parts: Vec<String> = hold
        .waiting_for
        .iter()
        .map(|pid| {
            let name = if Some(*pid) == me { "you".to_owned() } else { nickname(room, *pid) };
            let status = room.participants.iter().find(|p| p.id == *pid).map(|p| &p.status);
            let detail = match (status, progress.get(pid).map(|e| &e.delivery)) {
                (_, Some(Delivery::Download { bytes, total })) if *total > 0 && bytes < total => {
                    format!("downloading {}%", bytes * 100 / total)
                }
                (Some(MediaStatus::Stalled), _) => "buffering".into(),
                (Some(MediaStatus::Loading), _) => "loading".into(),
                _ => String::new(),
            };
            if detail.is_empty() { name } else { format!("{name} ({detail})") }
        })
        .collect();
    format!("Waiting for: {}", parts.join(", "))
}

pub fn render_notice(actor: Option<&str>, action: &NoticeAction, room: &RoomState) -> String {
    let who = actor.unwrap_or("Someone");
    match action {
        NoticeAction::Paused => format!("{who} paused"),
        NoticeAction::Resumed => format!("{who} resumed"),
        NoticeAction::Seeked { to_ms } => format!("{who} jumped to {}", fmt_time(*to_ms)),
        NoticeAction::SpeedChanged { speed } => format!("{who} set speed to {speed}×"),
        NoticeAction::ItemChanged { item_id } => format!("Now playing: {}", title(room, *item_id)),
        NoticeAction::PlaylistAdded { item_ids } => match item_ids.len() {
            1 => format!("{who} added {}", title(room, item_ids[0])),
            n => format!("{who} added {n} items"),
        },
        NoticeAction::PlaylistRemoved { .. } => format!("{who} removed an item"),
        NoticeAction::PlaylistMoved { .. } => format!("{who} reordered the playlist"),
        NoticeAction::Joined => format!("{who} joined"),
        NoticeAction::Left => format!("{who} left"),
        NoticeAction::Kicked { participant_id } => {
            format!("{} was removed by {who}", nickname(room, *participant_id))
        }
        NoticeAction::ControlModeChanged { mode } => match mode {
            ControlMode::Everyone => "Everyone can control playback".into(),
            ControlMode::HostOnly => "Only the host can control playback".into(),
            ControlMode::Selected => "Only selected participants can control playback".into(),
        },
        NoticeAction::CanControlChanged { participant_id, allowed } => {
            let n = nickname(room, *participant_id);
            if *allowed {
                format!("{n} can now control playback")
            } else {
                format!("{n} can no longer control playback")
            }
        }
        NoticeAction::HoldStarted { hold } => {
            let names: Vec<String> = hold.waiting_for.iter().map(|p| nickname(room, *p)).collect();
            format!("Waiting for {}", names.join(", "))
        }
        NoticeAction::HoldEnded => "Everyone is ready".into(),
        NoticeAction::ItemSkipped { item_id, reason } => match reason {
            SkipReason::FileMissing => {
                format!("Skipped {}: the file is missing", title(room, *item_id))
            }
            SkipReason::AllFailed => {
                format!("Skipped {}: nobody could open it", title(room, *item_id))
            }
        },
        NoticeAction::DeliveryFellBack { participant_id } => {
            format!("{}: download failed, streaming instead", nickname(room, *participant_id))
        }
    }
}

#[cfg(test)]
mod tests {
    use sppy_proto::*;

    use super::*;

    fn room() -> RoomState {
        let p = |id, nick: &str| Participant {
            id: ParticipantId(id),
            nickname: nick.into(),
            is_host: false,
            can_control: false,
            connected: true,
            status: MediaStatus::Loading,
        };
        RoomState {
            room_id: RoomId::main(),
            participants: vec![p(1, "alice"), p(2, "bob")],
            control_mode: ControlMode::Everyone,
            playlist: vec![Item {
                item_id: ItemId(5),
                source: Source::Url { url: "u".into() },
                title: "Movie".into(),
                duration_ms: None,
                fingerprint: None,
                subtitles: vec![],
                added_by: ParticipantId(1),
            }],
            settings: RoomSettings { stall_hold_secs: 10, resume_buffer_secs: 10 },
        }
    }

    #[test]
    fn notice_rendering() {
        let r = room();
        assert_eq!(render_notice(Some("alice"), &NoticeAction::Paused, &r), "alice paused");
        assert_eq!(render_notice(Some("alice"), &NoticeAction::Resumed, &r), "alice resumed");
        assert_eq!(
            render_notice(Some("bob"), &NoticeAction::Seeked { to_ms: 3_725_000 }, &r),
            "bob jumped to 1:02:05"
        );
        let hold = Hold { reason: HoldReason::Stalled, waiting_for: vec![ParticipantId(2)] };
        assert_eq!(render_notice(None, &NoticeAction::HoldStarted { hold }, &r), "Waiting for bob");
        let hold = Hold {
            reason: HoldReason::ItemStart,
            waiting_for: vec![ParticipantId(1), ParticipantId(2)],
        };
        assert_eq!(
            render_notice(None, &NoticeAction::HoldStarted { hold }, &r),
            "Waiting for alice, bob"
        );
        assert_eq!(
            render_notice(None, &NoticeAction::ItemChanged { item_id: ItemId(5) }, &r),
            "Now playing: Movie"
        );
        assert_eq!(
            render_notice(
                Some("alice"),
                &NoticeAction::Kicked { participant_id: ParticipantId(2) },
                &r
            ),
            "bob was removed by alice"
        );
        assert_eq!(render_notice(None, &NoticeAction::Paused, &r), "Someone paused");
        assert_eq!(
            render_notice(Some("alice"), &NoticeAction::SpeedChanged { speed: 1.25 }, &r),
            "alice set speed to 1.25×"
        );
    }

    #[test]
    fn format_time() {
        assert_eq!(fmt_time(0), "0:00");
        assert_eq!(fmt_time(65_000), "1:05");
        assert_eq!(fmt_time(3_600_000), "1:00:00");
    }
}

#[cfg(test)]
mod v2_tests {
    use std::collections::HashMap;

    use sppy_proto::*;

    use super::*;

    #[test]
    fn osd_shows_only_what_matters() {
        let other = |a: NoticeAction| osd_worthy(&a, false);
        assert!(other(NoticeAction::Paused));
        assert!(other(NoticeAction::Resumed));
        assert!(other(NoticeAction::Seeked { to_ms: 1 }));
        assert!(other(NoticeAction::SpeedChanged { speed: 1.5 }));
        assert!(other(NoticeAction::ItemChanged { item_id: ItemId(1) }));
        assert!(other(NoticeAction::HoldStarted {
            hold: Hold { reason: HoldReason::Stalled, waiting_for: vec![] }
        }));
        assert!(other(NoticeAction::HoldEnded));
        assert!(!other(NoticeAction::Joined));
        assert!(!other(NoticeAction::Left));
        assert!(!other(NoticeAction::PlaylistAdded { item_ids: vec![] }));
        assert!(!other(NoticeAction::PlaylistMoved { item_id: ItemId(1) }));
        assert!(!other(NoticeAction::ControlModeChanged { mode: ControlMode::Everyone }));
        // My own actions are already visible in my player.
        assert!(!osd_worthy(&NoticeAction::Paused, true));
        assert!(!osd_worthy(&NoticeAction::Seeked { to_ms: 1 }, true));
        // ...except the room changing item (the player is about to reload).
        assert!(osd_worthy(&NoticeAction::ItemChanged { item_id: ItemId(1) }, true));
    }

    fn room() -> RoomState {
        let p = |id, nick: &str, status| Participant {
            id: ParticipantId(id),
            nickname: nick.into(),
            is_host: false,
            can_control: false,
            connected: true,
            status,
        };
        RoomState {
            room_id: RoomId::main(),
            participants: vec![
                p(1, "me", MediaStatus::Loading),
                p(2, "yaroslav", MediaStatus::Stalled),
                p(3, "ann", MediaStatus::Loading),
            ],
            control_mode: ControlMode::Everyone,
            playlist: vec![],
            settings: RoomSettings { stall_hold_secs: 10, resume_buffer_secs: 10 },
        }
    }

    #[test]
    fn waiting_line_explains_each_participant() {
        let hold = Hold {
            reason: HoldReason::ItemStart,
            waiting_for: vec![ParticipantId(1), ParticipantId(2), ParticipantId(3)],
        };
        let mut progress = HashMap::new();
        progress.insert(
            ParticipantId(1),
            ProgressEntry {
                participant_id: ParticipantId(1),
                item_id: ItemId(1),
                delivery: Delivery::Download { bytes: 45, total: 100 },
                drift_ms: None,
            },
        );
        assert_eq!(
            waiting_line(&hold, &room(), &progress, Some(ParticipantId(1))),
            "Waiting for: you (downloading 45%), yaroslav (buffering), ann (loading)"
        );
    }
}
