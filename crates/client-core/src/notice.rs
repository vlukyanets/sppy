//! Human-readable text for server notices (also shown as OSD).

use sppy_proto::{
    ControlMode, ItemId, LagPolicy, NoticeAction, ParticipantId, Repeat, RoomState, SkipReason,
};

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
        | NoticeAction::SpeedChanged { .. }
        | NoticeAction::ReadyChanged { .. } => !actor_is_me,
        NoticeAction::ItemChanged { .. }
        | NoticeAction::HoldStarted { .. }
        | NoticeAction::HoldEnded
        | NoticeAction::ItemSkipped { .. }
        | NoticeAction::WentBackFor { .. }
        | NoticeAction::Kicked { .. } => true,
        NoticeAction::PlaylistAdded { .. }
        | NoticeAction::PlaylistRemoved { .. }
        | NoticeAction::PlaylistMoved { .. }
        | NoticeAction::PlaylistShuffled
        | NoticeAction::RepeatChanged { .. }
        | NoticeAction::Joined
        | NoticeAction::Left
        | NoticeAction::ControlModeChanged { .. }
        | NoticeAction::LagPolicyChanged { .. }
        | NoticeAction::ReadyCheckChanged { .. }
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
    use sppy_proto::{Delivery, HoldReason, MediaStatus};
    if hold.waiting_for.is_empty() {
        return "Waiting for a player to open the item".into();
    }
    let wants_ready = room.ready_check && hold.reason == HoldReason::ItemStart;
    let line = |pid: &ParticipantId| {
        let name = if Some(*pid) == me { "you".to_owned() } else { nickname(room, *pid) };
        let p = room.participants.iter().find(|p| p.id == *pid);
        let detail = match (p.map(|p| &p.status), progress.get(pid).map(|e| &e.delivery)) {
            (_, Some(Delivery::Download { bytes, total })) if *total > 0 && bytes < total => {
                format!("downloading {}%", bytes * 100 / total)
            }
            (Some(MediaStatus::Stalled), _) => "buffering".into(),
            (Some(MediaStatus::Loading), _) => "loading".into(),
            _ if wants_ready && p.is_some_and(|p| !p.ready) => "not ready".into(),
            _ => String::new(),
        };
        (detail.is_empty(), if detail.is_empty() { name } else { format!("{name} ({detail})") })
    };
    let all: Vec<(bool, String)> = hold.waiting_for.iter().map(line).collect();
    // Only those the room still waits for; everyone if that leaves nobody.
    let blocking: Vec<&str> =
        all.iter().filter(|(done, _)| !done).map(|(_, text)| text.as_str()).collect();
    let parts: Vec<&str> = if blocking.is_empty() {
        all.iter().map(|(_, text)| text.as_str()).collect()
    } else {
        blocking
    };
    format!("Waiting for: {}", parts.join(", "))
}

pub fn render_notice(actor: Option<&str>, action: &NoticeAction, room: &RoomState) -> String {
    let who = actor.unwrap_or("Someone");
    match action {
        NoticeAction::Paused => format!("{who} paused"),
        NoticeAction::Resumed => format!("{who} resumed"),
        NoticeAction::Seeked { to_ms } => format!("{who} jumped to {}", fmt_time(*to_ms)),
        NoticeAction::SpeedChanged { speed } => format!("{who} set speed to {speed}×"),
        NoticeAction::ItemChanged { item_id } if room.ready_check => {
            format!("Now playing: {}. Press Ready in sppy", title(room, *item_id))
        }
        NoticeAction::ItemChanged { item_id } => format!("Now playing: {}", title(room, *item_id)),
        NoticeAction::PlaylistAdded { item_ids } => match item_ids.len() {
            1 => format!("{who} added {}", title(room, item_ids[0])),
            n => format!("{who} added {n} items"),
        },
        NoticeAction::PlaylistRemoved { .. } => format!("{who} removed an item"),
        NoticeAction::PlaylistMoved { .. } => format!("{who} reordered the playlist"),
        NoticeAction::PlaylistShuffled => format!("{who} shuffled the playlist"),
        NoticeAction::RepeatChanged { repeat } => match repeat {
            Repeat::Off => format!("{who} turned repeat off"),
            Repeat::All => format!("{who} set the playlist to repeat"),
            Repeat::One => format!("{who} set the current item to repeat"),
        },
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
        NoticeAction::LagPolicyChanged { policy } => match policy {
            LagPolicy::CatchUpAlone => "Whoever falls behind now catches up alone".into(),
            LagPolicy::FollowSlowest => "The room now waits for whoever falls behind".into(),
        },
        NoticeAction::WentBackFor { participant_id, back_ms } => format!(
            "Went back {:.1} s for {}",
            *back_ms as f64 / 1000.0,
            nickname(room, *participant_id)
        ),
        NoticeAction::ReadyChanged { ready: true } => format!("{who} is ready"),
        NoticeAction::ReadyChanged { ready: false } => format!("{who} is not ready"),
        NoticeAction::ReadyCheckChanged { on: true } => {
            "New items now start once everyone has pressed Ready".into()
        }
        NoticeAction::ReadyCheckChanged { on: false } => {
            "New items now start once everyone has loaded them".into()
        }
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

/// Why the server refused us. A different protocol version gets its own explanation; the
/// server's own words otherwise.
pub fn reject_text(code: &sppy_proto::ErrorCode, server_message: &str) -> String {
    match code {
        sppy_proto::ErrorCode::VersionMismatch { server_protocol } => format!(
            "This server runs a different version of sppy (protocol v{server_protocol}, this app \
             v{}). Use the same sppy version on both sides.",
            sppy_proto::PROTOCOL_VERSION
        ),
        _ => server_message.to_owned(),
    }
}

/// What to tell the user when the server refused a request.
pub fn denied_text(code: &sppy_proto::ErrorCode) -> String {
    use sppy_proto::ErrorCode;
    match code {
        ErrorCode::NotPermitted => crate::session::NO_PERMISSION.to_owned(),
        ErrorCode::NotHost => "Only the host can do that".to_owned(),
        ErrorCode::MediaFolder { problem } => folder_problem_text(*problem).to_owned(),
        ErrorCode::PlaylistFull { max_items } => {
            format!("The playlist is full ({max_items} items). Remove some items first")
        }
        other => format!("Request rejected: {other:?}"),
    }
}

/// What to tell the host when a folder change is refused.
pub fn folder_problem_text(problem: sppy_proto::MediaFolderProblem) -> &'static str {
    use sppy_proto::MediaFolderProblem::*;
    match problem {
        ChangesDisabled => "This server does not allow changing its folders from the app",
        NotAbsolute => "Enter the full path of a folder on the server",
        NotFound => "There is no such folder on the server",
        NotAFolder => "That path is a file, not a folder",
        AlreadyShared => "That folder is already shared",
        NameTaken => "Another shared folder has that name",
        UnknownFolder => "No shared folder with that name",
        CannotSave => "The server could not save its config file; see the server log",
        OutsideMediaRoot => "Only folders inside the server's media_root can be shared",
        NoMediaRoot => "To share folders from the app, set media_root in the server's server.toml",
    }
}

#[cfg(test)]
mod tests {
    use sppy_proto::*;

    use super::*;

    #[test]
    fn full_playlist_says_what_to_do() {
        assert_eq!(
            denied_text(&ErrorCode::PlaylistFull { max_items: 1000 }),
            "The playlist is full (1000 items). Remove some items first"
        );
    }

    #[test]
    fn lag_notices_say_what_happened() {
        let r = room();
        let went_back =
            NoticeAction::WentBackFor { participant_id: ParticipantId(2), back_ms: 1_540 };
        assert_eq!(render_notice(None, &went_back, &r), "Went back 1.5 s for bob");
        let follow = NoticeAction::LagPolicyChanged { policy: LagPolicy::FollowSlowest };
        assert_eq!(
            render_notice(Some("alice"), &follow, &r),
            "The room now waits for whoever falls behind"
        );
        let alone = NoticeAction::LagPolicyChanged { policy: LagPolicy::CatchUpAlone };
        assert_eq!(
            render_notice(Some("alice"), &alone, &r),
            "Whoever falls behind now catches up alone"
        );
    }

    #[test]
    fn playlist_mode_notices() {
        let r = room();
        let said = |a: NoticeAction| render_notice(Some("bob"), &a, &r);
        assert_eq!(said(NoticeAction::PlaylistShuffled), "bob shuffled the playlist");
        assert_eq!(
            said(NoticeAction::RepeatChanged { repeat: Repeat::All }),
            "bob set the playlist to repeat"
        );
        assert_eq!(
            said(NoticeAction::RepeatChanged { repeat: Repeat::One }),
            "bob set the current item to repeat"
        );
        assert_eq!(
            said(NoticeAction::RepeatChanged { repeat: Repeat::Off }),
            "bob turned repeat off"
        );
        assert!(!osd_worthy(&NoticeAction::PlaylistShuffled, false));
    }

    #[test]
    fn ready_notices_say_who_and_what_to_do() {
        let mut r = room();
        let ready = NoticeAction::ReadyChanged { ready: true };
        assert_eq!(render_notice(Some("bob"), &ready, &r), "bob is ready");
        let not = NoticeAction::ReadyChanged { ready: false };
        assert_eq!(render_notice(Some("bob"), &not, &r), "bob is not ready");
        assert!(osd_worthy(&ready, false) && !osd_worthy(&ready, true));
        let on = NoticeAction::ReadyCheckChanged { on: true };
        assert_eq!(
            render_notice(Some("alice"), &on, &r),
            "New items now start once everyone has pressed Ready"
        );
        let off = NoticeAction::ReadyCheckChanged { on: false };
        assert_eq!(
            render_notice(Some("alice"), &off, &r),
            "New items now start once everyone has loaded them"
        );
        assert!(!osd_worthy(&on, false));
        let changed = NoticeAction::ItemChanged { item_id: ItemId(5) };
        assert_eq!(render_notice(None, &changed, &r), "Now playing: Movie");
        r.ready_check = true;
        assert_eq!(render_notice(None, &changed, &r), "Now playing: Movie. Press Ready in sppy");
    }

    #[test]
    fn the_wait_names_who_has_not_pressed_ready() {
        let mut r = room();
        r.ready_check = true;
        r.participants[0].status = MediaStatus::Ready { duration_ms: None, title: None };
        r.participants[1].status = MediaStatus::Ready { duration_ms: None, title: None };
        r.participants[1].ready = true;
        let hold = Hold {
            reason: HoldReason::ItemStart,
            waiting_for: vec![ParticipantId(1), ParticipantId(2)],
        };
        let line = waiting_line(&hold, &r, &Default::default(), Some(ParticipantId(2)));
        assert_eq!(line, "Waiting for: alice (not ready)");
    }

    /// Nobody's player was open when the item started: the room waits for the first one.
    #[test]
    fn an_empty_wait_says_what_it_waits_for() {
        let hold = Hold { reason: HoldReason::ItemStart, waiting_for: vec![] };
        let line = waiting_line(&hold, &room(), &Default::default(), Some(ParticipantId(2)));
        assert_eq!(line, "Waiting for a player to open the item");
    }

    fn room() -> RoomState {
        let p = |id, nick: &str| Participant {
            id: ParticipantId(id),
            nickname: nick.into(),
            is_host: false,
            can_control: false,
            connected: true,
            status: MediaStatus::Loading,
            ready: false,
        };
        RoomState {
            room_id: RoomId::main(),
            participants: vec![p(1, "alice"), p(2, "bob")],
            control_mode: ControlMode::Everyone,
            lag_policy: LagPolicy::default(),
            ready_check: false,
            repeat: Default::default(),
            playlist: vec![Item {
                item_id: ItemId(5),
                source: Source::Url { url: "u".into() },
                title: "Movie".into(),
                duration_ms: None,
                fingerprint: None,
                subtitles: vec![],
                added_by: ParticipantId(1),
                watched: false,
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
        assert!(!other(NoticeAction::LagPolicyChanged { policy: LagPolicy::FollowSlowest }));
        // A jump back everyone sees in their player.
        assert!(other(NoticeAction::WentBackFor { participant_id: ParticipantId(2), back_ms: 1 }));
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
            ready: false,
        };
        RoomState {
            room_id: RoomId::main(),
            participants: vec![
                p(1, "me", MediaStatus::Loading),
                p(2, "yaroslav", MediaStatus::Stalled),
                p(3, "ann", MediaStatus::Loading),
            ],
            control_mode: ControlMode::Everyone,
            lag_policy: LagPolicy::default(),
            ready_check: false,
            repeat: Default::default(),
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
