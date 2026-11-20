use super::*;
use sppy_proto::*;

pub(super) fn config() -> RoomConfig {
    RoomConfig {
        max_participants: 8,
        resume_grace_ms: 60_000,
        settings: RoomSettings { stall_hold_secs: 10, resume_buffer_secs: 10 },
    }
}

pub(super) fn room() -> Room {
    Room::new(config())
}

pub(super) fn join_as(room: &mut Room, now: u64, nick: &str, role: JoinRole) -> ParticipantId {
    room.join(
        now,
        JoinRequest { nickname: nick.into(), role, session_token: format!("sess-{nick}") },
    )
    .expect("join")
    .participant_id
}

pub(super) fn join(room: &mut Room, nick: &str) -> ParticipantId {
    join_as(room, 0, nick, JoinRole::Guest)
}

pub(super) fn participant(room: &Room, pid: ParticipantId) -> Option<Participant> {
    room.state().participants.into_iter().find(|p| p.id == pid)
}

mod participants {
    use super::*;

    #[test]
    fn join_sends_welcome_and_broadcasts_room() {
        let mut r = room();
        let joined = r
            .join(
                5,
                JoinRequest {
                    nickname: "alice".into(),
                    role: JoinRole::Guest,
                    session_token: "s".into(),
                },
            )
            .unwrap();
        assert_eq!(joined.participant_id, ParticipantId(1));
        match &joined.effects[0] {
            Effect::Send { to, msg: ServerMsg::Welcome(w) } => {
                assert_eq!(*to, ParticipantId(1));
                assert_eq!(w.participant_id, ParticipantId(1));
                assert!(!w.is_host);
                assert_eq!(w.session_token, "s");
                assert_eq!(w.server_time_ms, 5);
                assert_eq!(w.room.participants.len(), 1);
            }
            other => panic!("expected Welcome, got {other:?}"),
        }
        assert!(matches!(joined.effects[1], Effect::Broadcast { msg: ServerMsg::Room(_) }));
        assert!(matches!(
            joined.effects[2],
            Effect::Broadcast {
                msg: ServerMsg::Event(Event::Notice {
                    actor: Some(ParticipantId(1)),
                    action: NoticeAction::Joined
                })
            }
        ));
    }

    #[test]
    fn nickname_taken_case_insensitive() {
        let mut r = room();
        join(&mut r, "alice");
        let err = r
            .join(
                0,
                JoinRequest {
                    nickname: "Alice".into(),
                    role: JoinRole::Guest,
                    session_token: "x".into(),
                },
            )
            .unwrap_err();
        assert_eq!(err, ErrorCode::NicknameTaken);
    }

    #[test]
    fn nickname_length_rules() {
        let mut r = room();
        for bad in ["".to_string(), "a".repeat(33)] {
            let err = r
                .join(
                    0,
                    JoinRequest { nickname: bad, role: JoinRole::Guest, session_token: "x".into() },
                )
                .unwrap_err();
            assert_eq!(err, ErrorCode::InvalidRequest);
        }
        assert!(
            r.join(
                0,
                JoinRequest {
                    nickname: "a".repeat(32),
                    role: JoinRole::Guest,
                    session_token: "x".into()
                }
            )
            .is_ok()
        );
    }

    #[test]
    fn room_full() {
        let mut r = room();
        for i in 0..8 {
            join(&mut r, &format!("u{i}"));
        }
        let err = r
            .join(
                0,
                JoinRequest {
                    nickname: "late".into(),
                    role: JoinRole::Guest,
                    session_token: "x".into(),
                },
            )
            .unwrap_err();
        assert_eq!(err, ErrorCode::RoomFull);
    }

    #[test]
    fn disconnect_keeps_participant_until_grace() {
        let mut r = room();
        let a = join(&mut r, "alice");
        r.disconnected(1000, a);
        assert!(!participant(&r, a).unwrap().connected);
        r.tick(1000 + 59_999);
        assert!(participant(&r, a).is_some());
        assert!(r.session_valid("sess-alice"));
        r.tick(1000 + 60_000);
        assert!(participant(&r, a).is_none());
        assert!(!r.session_valid("sess-alice"));
    }

    #[test]
    fn resume_within_grace_restores_identity() {
        let mut r = room();
        let a = join(&mut r, "alice");
        let b = join(&mut r, "bob");
        r.test_set_can_control(b, true);
        r.disconnected(1000, b);
        let joined = r.resume(2000, "sess-bob").unwrap();
        assert_eq!(joined.participant_id, b);
        let p = participant(&r, b).unwrap();
        assert!(p.connected);
        assert!(p.can_control);
        let ids: Vec<_> = r.state().participants.iter().map(|p| p.id).collect();
        assert_eq!(ids, vec![a, b]);
        assert!(matches!(&joined.effects[0], Effect::Send { msg: ServerMsg::Welcome(_), .. }));
    }

    #[test]
    fn resume_after_grace_fails() {
        let mut r = room();
        let a = join(&mut r, "alice");
        r.disconnected(0, a);
        r.tick(60_000);
        assert_eq!(r.resume(60_001, "sess-alice").unwrap_err(), ErrorCode::SessionExpired);
    }

    #[test]
    fn nickname_reserved_during_grace() {
        let mut r = room();
        let a = join(&mut r, "alice");
        r.disconnected(0, a);
        let err = r
            .join(
                1,
                JoinRequest {
                    nickname: "alice".into(),
                    role: JoinRole::Guest,
                    session_token: "x".into(),
                },
            )
            .unwrap_err();
        assert_eq!(err, ErrorCode::NicknameTaken);
    }

    #[test]
    fn leave_removes_immediately() {
        let mut r = room();
        let a = join(&mut r, "alice");
        let effects = r.leave(10, a);
        assert!(participant(&r, a).is_none());
        assert!(!r.session_valid("sess-alice"));
        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::Broadcast {
                msg: ServerMsg::Event(Event::Notice { action: NoticeAction::Left, .. })
            }
        )));
    }

    #[test]
    fn host_role() {
        let mut r = room();
        let joined = r
            .join(
                0,
                JoinRequest {
                    nickname: "h".into(),
                    role: JoinRole::Host,
                    session_token: "s".into(),
                },
            )
            .unwrap();
        let Effect::Send { msg: ServerMsg::Welcome(w), .. } = &joined.effects[0] else { panic!() };
        assert!(w.is_host);
        assert!(r.is_host(joined.participant_id));
        assert!(participant(&r, joined.participant_id).unwrap().is_host);
    }
}

pub(super) fn host(room: &mut Room) -> ParticipantId {
    join_as(room, 0, "host", JoinRole::Host)
}

/// Asserts the exact "denied + resync" triple sent only to `to`.
pub(super) fn assert_denied(effects: &[Effect], to: ParticipantId, req_id: ReqId, code: ErrorCode) {
    assert_eq!(effects.len(), 3, "{effects:?}");
    assert_eq!(effects[0], Effect::Send { to, msg: ServerMsg::Denied { req_id, code } });
    assert!(matches!(&effects[1], Effect::Send { to: t, msg: ServerMsg::Playback(_) } if *t == to));
    assert!(matches!(&effects[2], Effect::Send { to: t, msg: ServerMsg::Room(_) } if *t == to));
}

pub(super) fn has_notice(effects: &[Effect], pred: impl Fn(&NoticeAction) -> bool) -> bool {
    effects.iter().any(|e| matches!(
        e,
        Effect::Broadcast { msg: ServerMsg::Event(Event::Notice { action, .. }) } if pred(action)
    ))
}

mod permissions {
    use super::*;

    fn set_mode(r: &mut Room, h: ParticipantId, mode: ControlMode) -> Vec<Effect> {
        r.command(0, h, ClientMsg::SetControlMode { req_id: 1, mode })
    }

    #[test]
    fn host_always_controls() {
        let mut r = room();
        let h = host(&mut r);
        set_mode(&mut r, h, ControlMode::HostOnly);
        assert!(r.can_control(h));
    }

    #[test]
    fn everyone_mode() {
        let mut r = room();
        let h = host(&mut r);
        let g = join(&mut r, "guest");
        set_mode(&mut r, h, ControlMode::Everyone);
        assert!(r.can_control(g));
        set_mode(&mut r, h, ControlMode::HostOnly);
        assert!(!r.can_control(g));
    }

    #[test]
    fn selected_mode_requires_grant() {
        let mut r = room();
        let h = host(&mut r);
        let g = join(&mut r, "guest");
        set_mode(&mut r, h, ControlMode::Selected);
        assert!(!r.can_control(g));
        let effects = r.command(
            0,
            h,
            ClientMsg::SetCanControl { req_id: 2, participant_id: g, allowed: true },
        );
        assert!(r.can_control(g));
        assert!(participant(&r, g).unwrap().can_control);
        assert!(has_notice(&effects, |a| matches!(
            a,
            NoticeAction::CanControlChanged { allowed: true, .. }
        )));
    }

    #[test]
    fn set_control_mode_by_guest_denied() {
        let mut r = room();
        host(&mut r);
        let g = join(&mut r, "guest");
        let effects = set_mode(&mut r, g, ControlMode::Everyone);
        assert_denied(&effects, g, 1, ErrorCode::NotHost);
    }

    #[test]
    fn set_can_control_unknown_participant() {
        let mut r = room();
        let h = host(&mut r);
        let effects = r.command(
            0,
            h,
            ClientMsg::SetCanControl {
                req_id: 4,
                participant_id: ParticipantId(99),
                allowed: true,
            },
        );
        assert_denied(&effects, h, 4, ErrorCode::InvalidRequest);
    }

    #[test]
    fn kick() {
        let mut r = room();
        let h = host(&mut r);
        let g = join(&mut r, "guest");
        let effects = r.command(0, h, ClientMsg::Kick { req_id: 3, participant_id: g });
        assert!(effects.contains(&Effect::Close { to: g, reason: CloseReason::Kicked }));
        assert!(participant(&r, g).is_none());
        assert!(!r.session_valid("sess-guest"));
        assert!(has_notice(&effects, |a| *a == NoticeAction::Kicked { participant_id: g }));
    }

    #[test]
    fn kick_by_guest_denied() {
        let mut r = room();
        let h = host(&mut r);
        let g = join(&mut r, "guest");
        let effects = r.command(0, g, ClientMsg::Kick { req_id: 5, participant_id: h });
        assert_denied(&effects, g, 5, ErrorCode::NotHost);
    }

    #[test]
    fn control_mode_change_broadcasts_room_and_notice() {
        let mut r = room();
        let h = host(&mut r);
        let effects = set_mode(&mut r, h, ControlMode::HostOnly);
        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::Broadcast { msg: ServerMsg::Room(s) } if s.control_mode == ControlMode::HostOnly
        )));
        assert!(has_notice(&effects, |a| *a
            == NoticeAction::ControlModeChanged { mode: ControlMode::HostOnly }));
        assert!(effects.contains(&Effect::Persist));
    }

    #[test]
    fn io_layer_messages_are_invalid_here() {
        let mut r = room();
        let h = host(&mut r);
        let effects = r.command(0, h, ClientMsg::RotateInvite { req_id: 6 });
        assert_denied(&effects, h, 6, ErrorCode::InvalidRequest);
        assert!(r.command(0, h, ClientMsg::GetLibrary).is_empty());
    }
}

pub(super) fn url_item(id: u64, duration_ms: Option<u64>) -> Item {
    Item {
        item_id: ItemId(id),
        source: Source::Url { url: format!("https://example.com/{id}") },
        title: format!("item {id}"),
        duration_ms,
        fingerprint: None,
        subtitles: vec![],
        added_by: ParticipantId(1),
    }
}

pub(super) fn last_playback(effects: &[Effect]) -> PlaybackState {
    effects
        .iter()
        .rev()
        .find_map(|e| match e {
            Effect::Broadcast { msg: ServerMsg::Playback(p) } => Some(p.clone()),
            _ => None,
        })
        .expect("a Playback broadcast")
}

mod playback {
    use super::*;

    /// Host + guest, one 60 s item, paused at 0.
    fn setup(duration_ms: Option<u64>) -> (Room, ParticipantId, ParticipantId) {
        let mut r = room();
        let h = host(&mut r);
        let g = join(&mut r, "guest");
        r.test_set_current(url_item(1, duration_ms));
        (r, h, g)
    }

    #[test]
    fn pause_rebases_position() {
        let (mut r, h, _) = setup(None);
        r.command(1000, h, ClientMsg::Play { req_id: 1, position_ms: None });
        let effects = r.command(4000, h, ClientMsg::Pause { req_id: 2, position_ms: None });
        let p = last_playback(&effects);
        assert_eq!(p.position_ms, 3000);
        assert_eq!(p.at_server_ms, 4000);
        assert!(p.paused);
        assert_eq!(p.seq, 2);
        assert_eq!(p.cause, Some(Cause { participant_id: h, req_id: 2 }));
        assert!(has_notice(&effects, |a| *a == NoticeAction::Paused));
        assert!(effects.contains(&Effect::Persist));
    }

    #[test]
    fn pause_uses_client_position() {
        let (mut r, h, _) = setup(None);
        r.command(1000, h, ClientMsg::Play { req_id: 1, position_ms: None });
        let effects = r.command(4000, h, ClientMsg::Pause { req_id: 2, position_ms: Some(2950) });
        assert_eq!(last_playback(&effects).position_ms, 2950);
    }

    #[test]
    fn play_notice_and_state() {
        let (mut r, h, _) = setup(None);
        let effects = r.command(1000, h, ClientMsg::Play { req_id: 1, position_ms: Some(500) });
        let p = last_playback(&effects);
        assert!(!p.paused);
        assert_eq!((p.position_ms, p.at_server_ms), (500, 1000));
        assert!(has_notice(&effects, |a| *a == NoticeAction::Resumed));
    }

    #[test]
    fn seek_while_playing() {
        let (mut r, h, _) = setup(None);
        r.command(1000, h, ClientMsg::Play { req_id: 1, position_ms: None });
        let effects = r.command(5000, h, ClientMsg::Seek { req_id: 2, position_ms: 30_000 });
        let p = last_playback(&effects);
        assert_eq!((p.position_ms, p.at_server_ms, p.paused), (30_000, 5000, false));
        assert!(has_notice(&effects, |a| *a == NoticeAction::Seeked { to_ms: 30_000 }));
    }

    #[test]
    fn seek_beyond_duration_clamps() {
        let (mut r, h, _) = setup(Some(60_000));
        let effects = r.command(0, h, ClientMsg::Seek { req_id: 1, position_ms: 90_000 });
        assert_eq!(last_playback(&effects).position_ms, 60_000);
    }

    #[test]
    fn set_speed() {
        let (mut r, h, _) = setup(None);
        r.command(0, h, ClientMsg::Play { req_id: 1, position_ms: None });
        let effects = r.command(2000, h, ClientMsg::SetSpeed { req_id: 2, speed: 1.5 });
        let p = last_playback(&effects);
        assert_eq!((p.speed, p.position_ms, p.at_server_ms), (1.5, 2000, 2000));
        assert!(has_notice(&effects, |a| *a == NoticeAction::SpeedChanged { speed: 1.5 }));
    }

    #[test]
    fn speed_out_of_range_denied() {
        let (mut r, h, _) = setup(None);
        for (i, speed) in [0.1, 5.0, f64::NAN].into_iter().enumerate() {
            let effects = r.command(0, h, ClientMsg::SetSpeed { req_id: i as u32, speed });
            assert_denied(&effects, h, i as u32, ErrorCode::InvalidRequest);
        }
    }

    #[test]
    fn no_current_item_denied() {
        let mut r = room();
        let h = host(&mut r);
        let effects = r.command(0, h, ClientMsg::Play { req_id: 1, position_ms: None });
        assert_denied(&effects, h, 1, ErrorCode::InvalidRequest);
    }

    #[test]
    fn guest_without_rights_denied_gets_state() {
        let (mut r, h, g) = setup(None);
        r.command(0, h, ClientMsg::SetControlMode { req_id: 1, mode: ControlMode::HostOnly });
        let effects = r.command(0, g, ClientMsg::Pause { req_id: 7, position_ms: None });
        assert_denied(&effects, g, 7, ErrorCode::NotPermitted);
    }

    #[test]
    fn pause_during_hold_kept_after_hold() {
        let (mut r, h, _) = setup(None);
        r.command(0, h, ClientMsg::Play { req_id: 1, position_ms: None });
        r.test_set_hold(Some(Hold { reason: HoldReason::ItemStart, waiting_for: vec![h] }));
        r.command(100, h, ClientMsg::Pause { req_id: 2, position_ms: None });
        r.test_set_hold(None);
        assert!(r.playback().paused);
        assert!(!r.playback().is_playing());
    }
}

pub(super) fn file_info(id: &str) -> FileInfo {
    FileInfo {
        file_id: FileId(id.into()),
        title: format!("{id}.mkv"),
        fingerprint: None,
        duration_ms: None,
        subtitles: vec![],
    }
}

pub(super) fn file_src(id: &str) -> Source {
    Source::File { file_id: FileId(id.into()) }
}

pub(super) fn add(
    r: &mut Room,
    now: u64,
    pid: ParticipantId,
    sources: Vec<Source>,
    after: Option<ItemId>,
) -> Vec<Effect> {
    r.command(now, pid, ClientMsg::PlaylistAdd { req_id: 100, sources, after })
}

pub(super) fn playlist_ids(r: &Room) -> Vec<u64> {
    r.state().playlist.iter().map(|i| i.item_id.0).collect()
}

mod playlist {
    use super::*;

    fn setup() -> (Room, ParticipantId) {
        let mut r = room();
        let h = host(&mut r);
        r.set_library(0, vec![file_info("a"), file_info("b"), file_info("c")]);
        (r, h)
    }

    #[test]
    fn add_unknown_file_denied() {
        let (mut r, h) = setup();
        let effects = r.command(
            0,
            h,
            ClientMsg::PlaylistAdd { req_id: 1, sources: vec![file_src("zzz")], after: None },
        );
        assert_denied(&effects, h, 1, ErrorCode::UnknownFile);
        assert!(playlist_ids(&r).is_empty());
    }

    #[test]
    fn add_to_empty_makes_current_paused() {
        let (mut r, h) = setup();
        let effects = add(&mut r, 0, h, vec![file_src("a")], None);
        let p = r.playback();
        assert_eq!(p.item_id, Some(ItemId(1)));
        assert!(p.paused);
        assert_eq!(p.position_ms, 0);
        assert_eq!(p.hold.as_ref().unwrap().reason, HoldReason::ItemStart);
        let item = &r.state().playlist[0];
        assert_eq!(item.title, "a.mkv");
        assert_eq!(item.added_by, h);
        assert!(has_notice(&effects, |a| *a
            == NoticeAction::PlaylistAdded { item_ids: vec![ItemId(1)] }));
        assert!(effects.contains(&Effect::Persist));
    }

    /// "Now playing" names the item, so clients must know the item before the notice.
    #[test]
    fn the_new_playlist_goes_out_before_now_playing() {
        let (mut r, h) = setup();
        let effects = add(&mut r, 0, h, vec![file_src("a")], None);
        let room_at = effects.iter().position(|e| {
            matches!(e, Effect::Broadcast { msg: ServerMsg::Room(s) } if !s.playlist.is_empty())
        });
        let notice_at = effects.iter().position(|e| {
            matches!(
                e,
                Effect::Broadcast {
                    msg: ServerMsg::Event(Event::Notice {
                        action: NoticeAction::ItemChanged { .. },
                        ..
                    })
                }
            )
        });
        assert!(room_at.unwrap() < notice_at.unwrap(), "{effects:#?}");
    }

    #[test]
    fn add_url_uses_url_as_title() {
        let (mut r, h) = setup();
        add(&mut r, 0, h, vec![Source::Url { url: "https://x/y".into() }], None);
        assert_eq!(r.state().playlist[0].title, "https://x/y");
    }

    /// Without a place, items go to the end: a client adding twice in a row cannot know
    /// the items of its first request yet, so it must not have to name the last item.
    #[test]
    fn add_after_an_item_or_at_the_end() {
        let (mut r, h) = setup();
        add(&mut r, 0, h, vec![file_src("a"), file_src("b")], None); // [1, 2]
        add(&mut r, 0, h, vec![file_src("c")], Some(ItemId(1))); // [1, 3, 2]
        assert_eq!(playlist_ids(&r), vec![1, 3, 2]);
        add(&mut r, 0, h, vec![file_src("c")], None); // at the end
        assert_eq!(playlist_ids(&r), vec![1, 3, 2, 4]);
        let effects = r.command(
            0,
            h,
            ClientMsg::PlaylistAdd {
                req_id: 9,
                sources: vec![file_src("a")],
                after: Some(ItemId(77)),
            },
        );
        assert_denied(&effects, h, 9, ErrorCode::UnknownItem);
    }

    #[test]
    fn move_item() {
        let (mut r, h) = setup();
        add(&mut r, 0, h, vec![file_src("a"), file_src("b"), file_src("c")], None); // [1,2,3]
        let effects =
            r.command(0, h, ClientMsg::PlaylistMove { req_id: 1, item_id: ItemId(3), after: None });
        assert_eq!(playlist_ids(&r), vec![3, 1, 2]);
        assert!(has_notice(&effects, |a| *a == NoticeAction::PlaylistMoved { item_id: ItemId(3) }));
        r.command(
            0,
            h,
            ClientMsg::PlaylistMove { req_id: 2, item_id: ItemId(3), after: Some(ItemId(2)) },
        );
        assert_eq!(playlist_ids(&r), vec![1, 2, 3]);
        let effects = r.command(
            0,
            h,
            ClientMsg::PlaylistMove { req_id: 3, item_id: ItemId(2), after: Some(ItemId(2)) },
        );
        assert_denied(&effects, h, 3, ErrorCode::InvalidRequest);
    }

    #[test]
    fn remove_non_current() {
        let (mut r, h) = setup();
        add(&mut r, 0, h, vec![file_src("a"), file_src("b")], None);
        let effects = r.command(0, h, ClientMsg::PlaylistRemove { req_id: 1, item_id: ItemId(2) });
        assert_eq!(playlist_ids(&r), vec![1]);
        assert_eq!(r.playback().item_id, Some(ItemId(1)));
        assert!(
            has_notice(&effects, |a| *a == NoticeAction::PlaylistRemoved { item_id: ItemId(2) })
        );
        let effects = r.command(0, h, ClientMsg::PlaylistRemove { req_id: 2, item_id: ItemId(2) });
        assert_denied(&effects, h, 2, ErrorCode::UnknownItem);
    }

    #[test]
    fn remove_current_advances() {
        let (mut r, h) = setup();
        add(&mut r, 0, h, vec![file_src("a"), file_src("b")], None);
        let effects =
            r.command(500, h, ClientMsg::PlaylistRemove { req_id: 1, item_id: ItemId(1) });
        let p = r.playback();
        assert_eq!(p.item_id, Some(ItemId(2)));
        assert_eq!(p.position_ms, 0);
        assert!(!p.paused);
        assert_eq!(p.hold.as_ref().unwrap().reason, HoldReason::ItemStart);
        assert!(has_notice(&effects, |a| *a == NoticeAction::ItemChanged { item_id: ItemId(2) }));
        r.command(600, h, ClientMsg::PlaylistRemove { req_id: 2, item_id: ItemId(2) });
        assert_eq!(r.playback().item_id, None);
    }

    #[test]
    fn select_resets_statuses_and_holds() {
        let (mut r, h) = setup();
        let g = join(&mut r, "guest");
        let off = join(&mut r, "offline");
        r.disconnected(0, off);
        add(&mut r, 0, h, vec![file_src("a"), file_src("b")], None);
        r.test_set_status(h, MediaStatus::Ready { duration_ms: None, title: None });
        let effects =
            r.command(1000, h, ClientMsg::PlaylistSelect { req_id: 1, item_id: ItemId(2) });
        let p = last_playback(&effects);
        assert_eq!(p.item_id, Some(ItemId(2)));
        assert!(!p.paused);
        assert_eq!((p.position_ms, p.at_server_ms), (0, 1000));
        let hold = p.hold.unwrap();
        assert_eq!(hold.reason, HoldReason::ItemStart);
        assert_eq!(hold.waiting_for, vec![h, g]);
        assert!(r.state().participants.iter().all(|p| p.status == MediaStatus::Loading));
        let effects = r.command(0, h, ClientMsg::PlaylistSelect { req_id: 2, item_id: ItemId(9) });
        assert_denied(&effects, h, 2, ErrorCode::UnknownItem);
    }

    #[test]
    fn missing_file_skipped_on_change() {
        let (mut r, h) = setup();
        add(&mut r, 0, h, vec![file_src("a"), file_src("b"), file_src("c")], None);
        r.set_library(0, vec![file_info("a"), file_info("c")]);
        let effects = r.command(0, h, ClientMsg::PlaylistSelect { req_id: 1, item_id: ItemId(2) });
        assert_eq!(r.playback().item_id, Some(ItemId(3)));
        assert!(has_notice(&effects, |a| *a
            == NoticeAction::ItemSkipped { item_id: ItemId(2), reason: SkipReason::FileMissing }));
    }

    #[test]
    fn playlist_requires_control() {
        let (mut r, h) = setup();
        let g = join(&mut r, "guest");
        r.command(0, h, ClientMsg::SetControlMode { req_id: 1, mode: ControlMode::HostOnly });
        let effects = r.command(
            0,
            g,
            ClientMsg::PlaylistAdd { req_id: 2, sources: vec![file_src("a")], after: None },
        );
        assert_denied(&effects, g, 2, ErrorCode::NotPermitted);
    }

    #[test]
    fn duplicate_file_gets_distinct_item_ids() {
        let (mut r, h) = setup();
        add(&mut r, 0, h, vec![file_src("a"), file_src("a")], None);
        assert_eq!(playlist_ids(&r), vec![1, 2]);
    }

    #[test]
    fn set_library_updates_item_metadata() {
        let (mut r, h) = setup();
        add(&mut r, 0, h, vec![file_src("a")], None);
        let mut info = file_info("a");
        info.fingerprint = Some(Fingerprint { size: 5, hash: "00".repeat(32) });
        let effects = r.set_library(0, vec![info]);
        assert_eq!(r.state().playlist[0].fingerprint.as_ref().unwrap().size, 5);
        assert!(effects.iter().any(|e| matches!(e, Effect::Broadcast { msg: ServerMsg::Room(_) })));
    }
}

pub(super) fn ready() -> MediaStatus {
    MediaStatus::Ready { duration_ms: None, title: None }
}

pub(super) fn status(
    r: &mut Room,
    now: u64,
    pid: ParticipantId,
    item: u64,
    status: MediaStatus,
) -> Vec<Effect> {
    r.command(now, pid, ClientMsg::SetStatus { item_id: ItemId(item), status })
}

mod status {
    use super::*;

    /// Host + guest, two URL items, item 1 selected (hold ItemStart) and both Ready at t=0 → playing.
    fn playing_room() -> (Room, ParticipantId, ParticipantId) {
        let mut r = room();
        let h = host(&mut r);
        let g = join(&mut r, "guest");
        add(
            &mut r,
            0,
            h,
            vec![Source::Url { url: "u1".into() }, Source::Url { url: "u2".into() }],
            None,
        );
        r.command(0, h, ClientMsg::PlaylistSelect { req_id: 1, item_id: ItemId(1) });
        status(&mut r, 0, h, 1, ready());
        status(&mut r, 0, g, 1, ready());
        assert!(r.playback().is_playing());
        (r, h, g)
    }

    #[test]
    fn item_start_hold_released_when_all_ready() {
        let mut r = room();
        let h = host(&mut r);
        let g = join(&mut r, "guest");
        add(&mut r, 0, h, vec![Source::Url { url: "u1".into() }], None);
        r.command(0, h, ClientMsg::Play { req_id: 1, position_ms: None });
        status(&mut r, 100, h, 1, ready());
        assert!(r.playback().hold.is_some());
        let effects = status(&mut r, 2000, g, 1, ready());
        let p = last_playback(&effects);
        assert!(p.hold.is_none());
        assert!(p.is_playing());
        assert_eq!((p.position_ms, p.at_server_ms), (0, 2000));
        assert!(has_notice(&effects, |a| *a == NoticeAction::HoldEnded));
    }

    #[test]
    fn status_for_other_item_ignored() {
        let (mut r, h, _) = playing_room();
        assert!(status(&mut r, 0, h, 2, MediaStatus::Stalled).is_empty());
        assert_eq!(participant(&r, h).unwrap().status, ready());
    }

    #[test]
    fn disconnected_participant_does_not_block() {
        let mut r = room();
        let h = host(&mut r);
        let g = join(&mut r, "guest");
        add(&mut r, 0, h, vec![Source::Url { url: "u1".into() }], None);
        r.command(0, h, ClientMsg::Play { req_id: 1, position_ms: None });
        status(&mut r, 0, h, 1, ready());
        let effects = r.disconnected(500, g);
        assert!(r.playback().hold.is_none());
        assert!(has_notice(&effects, |a| *a == NoticeAction::HoldEnded));
    }

    #[test]
    fn player_closed_does_not_block() {
        let mut r = room();
        let h = host(&mut r);
        let g = join(&mut r, "guest");
        add(&mut r, 0, h, vec![Source::Url { url: "u1".into() }], None);
        r.command(0, h, ClientMsg::Play { req_id: 1, position_ms: None });
        status(&mut r, 0, h, 1, ready());
        status(&mut r, 0, g, 1, MediaStatus::PlayerClosed);
        assert!(r.playback().is_playing());
    }

    #[test]
    fn stall_shorter_than_threshold_no_hold() {
        let (mut r, _, g) = playing_room();
        status(&mut r, 1000, g, 1, MediaStatus::Stalled);
        r.tick(1000 + 9_999);
        assert!(r.playback().hold.is_none());
    }

    #[test]
    fn stall_hold_after_threshold() {
        let (mut r, _, g) = playing_room();
        status(&mut r, 1000, g, 1, MediaStatus::Stalled);
        let effects = r.tick(11_000);
        let p = last_playback(&effects);
        assert_eq!(p.hold, Some(Hold { reason: HoldReason::Stalled, waiting_for: vec![g] }));
        assert_eq!((p.position_ms, p.at_server_ms), (11_000, 11_000));
        assert!(has_notice(&effects, |a| matches!(a, NoticeAction::HoldStarted { .. })));
        assert_eq!(r.playback().expected_position_ms(20_000), 11_000);
    }

    #[test]
    fn stall_recovered_before_threshold_no_hold() {
        let (mut r, _, g) = playing_room();
        status(&mut r, 1000, g, 1, MediaStatus::Stalled);
        status(&mut r, 5000, g, 1, ready());
        r.tick(12_000);
        assert!(r.playback().hold.is_none());
    }

    #[test]
    fn stall_hold_disabled_when_zero() {
        let mut cfg = config();
        cfg.settings.stall_hold_secs = 0;
        let mut r = Room::new(cfg);
        let h = host(&mut r);
        add(&mut r, 0, h, vec![Source::Url { url: "u1".into() }], None);
        r.command(0, h, ClientMsg::Play { req_id: 1, position_ms: None });
        status(&mut r, 0, h, 1, ready());
        status(&mut r, 0, h, 1, MediaStatus::Stalled);
        r.tick(100_000);
        assert!(r.playback().hold.is_none());
    }

    #[test]
    fn hold_released_resumes_from_frozen_position() {
        let (mut r, _, g) = playing_room();
        status(&mut r, 1000, g, 1, MediaStatus::Stalled);
        r.tick(11_000);
        let effects = status(&mut r, 30_000, g, 1, ready());
        let p = last_playback(&effects);
        assert!(p.is_playing());
        assert_eq!((p.position_ms, p.at_server_ms), (11_000, 30_000));
    }

    #[test]
    fn ready_records_duration_and_url_title() {
        let (mut r, h, g) = playing_room();
        status(
            &mut r,
            0,
            h,
            1,
            MediaStatus::Ready { duration_ms: Some(60_000), title: Some("Clip".into()) },
        );
        status(
            &mut r,
            0,
            g,
            1,
            MediaStatus::Ready { duration_ms: Some(61_000), title: Some("Other".into()) },
        );
        let item = &r.state().playlist[0];
        assert_eq!(item.duration_ms, Some(61_000), "the largest reported duration");
        assert_eq!(item.title, "Clip");
        status(&mut r, 0, g, 1, MediaStatus::Ready { duration_ms: Some(1), title: None });
        assert_eq!(
            r.state().playlist[0].duration_ms,
            Some(61_000),
            "a smaller one changes nothing"
        );
    }

    #[test]
    fn item_ended_advances() {
        let (mut r, h, g) = playing_room();
        assert!(r.command(5000, h, ClientMsg::ItemEnded { item_id: ItemId(1) }).is_empty());
        let effects = r.command(5000, g, ClientMsg::ItemEnded { item_id: ItemId(1) });
        let p = last_playback(&effects);
        assert_eq!(p.item_id, Some(ItemId(2)));
        assert_eq!(p.hold.unwrap().reason, HoldReason::ItemStart);
    }

    #[test]
    fn an_inflated_duration_does_not_hold_up_the_end() {
        let (mut r, h, g) = playing_room();
        // Someone reports far more than the link really lasts.
        status(&mut r, 0, g, 1, MediaStatus::Ready { duration_ms: Some(600_000), title: None });
        r.command(30_000, h, ClientMsg::ItemEnded { item_id: ItemId(1) });
        assert_eq!(r.playback().item_id, Some(ItemId(1)));
        r.command(30_000, g, ClientMsg::ItemEnded { item_id: ItemId(1) });
        assert_eq!(r.playback().item_id, Some(ItemId(2)), "everyone reached the end");
    }

    #[test]
    fn item_ended_stale_item_ignored() {
        let (mut r, h, _) = playing_room();
        assert!(r.command(0, h, ClientMsg::ItemEnded { item_id: ItemId(2) }).is_empty());
        assert_eq!(r.playback().item_id, Some(ItemId(1)));
    }

    #[test]
    fn last_item_end_pauses_at_end() {
        let mut r = room();
        let h = host(&mut r);
        add(&mut r, 0, h, vec![Source::Url { url: "u1".into() }], None);
        r.command(0, h, ClientMsg::Play { req_id: 1, position_ms: None });
        status(&mut r, 0, h, 1, MediaStatus::Ready { duration_ms: Some(10_000), title: None });
        let effects = r.command(10_000, h, ClientMsg::ItemEnded { item_id: ItemId(1) });
        let p = last_playback(&effects);
        assert_eq!(p.item_id, Some(ItemId(1)));
        assert!(p.paused);
        assert_eq!(p.position_ms, 10_000);
        assert!(r.command(10_100, h, ClientMsg::ItemEnded { item_id: ItemId(1) }).is_empty());
    }

    #[test]
    fn all_failed_url_skipped() {
        let (mut r, h, g) = playing_room();
        status(&mut r, 0, h, 1, MediaStatus::Failed { message: "x".into() });
        assert_eq!(r.playback().item_id, Some(ItemId(1)));
        let effects = status(&mut r, 0, g, 1, MediaStatus::Failed { message: "y".into() });
        assert_eq!(r.playback().item_id, Some(ItemId(2)));
        assert!(has_notice(&effects, |a| *a
            == NoticeAction::ItemSkipped { item_id: ItemId(1), reason: SkipReason::AllFailed }));
    }

    #[test]
    fn progress_batched_once_per_second() {
        let (mut r, h, g) = playing_room();
        r.command(
            100,
            h,
            ClientMsg::Progress { item_id: ItemId(1), delivery: Delivery::Stream, drift_ms: None },
        );
        r.command(
            200,
            g,
            ClientMsg::Progress {
                item_id: ItemId(1),
                delivery: Delivery::Download { bytes: 1, total: 10 },
                drift_ms: None,
            },
        );
        let batch = |effects: &[Effect]| {
            effects.iter().find_map(|e| match e {
                Effect::Broadcast { msg: ServerMsg::ProgressBatch { entries } } => {
                    Some(entries.clone())
                }
                _ => None,
            })
        };
        let first = batch(&r.tick(1000)).expect("first batch");
        assert_eq!(first.len(), 2);
        r.command(
            1100,
            g,
            ClientMsg::Progress {
                item_id: ItemId(1),
                delivery: Delivery::Download { bytes: 5, total: 10 },
                drift_ms: None,
            },
        );
        assert!(batch(&r.tick(1500)).is_none(), "not within 1 s");
        assert!(batch(&r.tick(2000)).is_some());
        assert!(batch(&r.tick(3500)).is_none(), "nothing dirty");
    }

    #[test]
    fn delivery_fallback_notice() {
        let (mut r, _, g) = playing_room();
        r.command(
            0,
            g,
            ClientMsg::Progress {
                item_id: ItemId(1),
                delivery: Delivery::Download { bytes: 1, total: 10 },
                drift_ms: None,
            },
        );
        let effects = r.command(
            0,
            g,
            ClientMsg::Progress { item_id: ItemId(1), delivery: Delivery::Stream, drift_ms: None },
        );
        assert!(
            has_notice(&effects, |a| *a == NoticeAction::DeliveryFellBack { participant_id: g })
        );
    }

    #[test]
    fn chat_history_capped_at_50_and_in_welcome() {
        let mut r = room();
        let h = host(&mut r);
        let effects = r.command(7, h, ClientMsg::Chat { text: "hello".into() });
        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::Broadcast { msg: ServerMsg::Event(Event::Chat(c)) } if c.text == "hello" && c.nickname == "host" && c.at_server_ms == 7
        )));
        for i in 0..60 {
            r.command(1_000 * (i + 1), h, ClientMsg::Chat { text: format!("m{i}") });
        }
        let joined = r
            .join(
                100_000,
                JoinRequest {
                    nickname: "late".into(),
                    role: JoinRole::Guest,
                    session_token: "x".into(),
                },
            )
            .unwrap();
        let Effect::Send { msg: ServerMsg::Welcome(w), .. } = &joined.effects[0] else { panic!() };
        assert_eq!(w.chat_history.len(), 50);
        assert_eq!(w.chat_history.last().unwrap().text, "m59");
    }

    #[test]
    fn chat_over_500_chars_rejected() {
        let mut r = room();
        let h = host(&mut r);
        assert!(r.command(0, h, ClientMsg::Chat { text: "x".repeat(501) }).is_empty());
        assert!(r.command(0, h, ClientMsg::Chat { text: "   ".into() }).is_empty());
        assert!(!r.command(0, h, ClientMsg::Chat { text: "x".repeat(500) }).is_empty());
    }
}

mod persist {
    use super::*;

    #[test]
    fn persist_roundtrip() {
        let mut r = room();
        let h = host(&mut r);
        r.command(0, h, ClientMsg::SetControlMode { req_id: 1, mode: ControlMode::Selected });
        let urls = ["u1", "u2", "u3"].map(|u| Source::Url { url: u.into() }).to_vec();
        add(&mut r, 0, h, urls, None);
        r.command(0, h, ClientMsg::PlaylistSelect { req_id: 2, item_id: ItemId(2) });
        status(&mut r, 0, h, 2, ready());
        r.command(0, h, ClientMsg::Seek { req_id: 3, position_ms: 40_000 });
        let saved = r.persisted(2000);
        assert_eq!(saved.version, 1);
        assert_eq!(saved.position_ms, 42_000);

        let json = serde_json::to_string(&saved).unwrap();
        let back: PersistedState = serde_json::from_str(&json).unwrap();
        let mut restored = Room::restore(config(), back);
        assert_eq!(restored.state().playlist, r.state().playlist);
        assert_eq!(restored.state().control_mode, ControlMode::Selected);
        let p = restored.playback();
        assert_eq!(p.item_id, Some(ItemId(2)));
        assert_eq!(p.position_ms, 42_000);
        assert!(p.paused);
        assert!(p.hold.is_none());

        let h2 = host(&mut restored);
        add(&mut restored, 0, h2, vec![Source::Url { url: "u4".into() }], None);
        assert_eq!(playlist_ids(&restored).last(), Some(&4), "ids go on after a restore");
    }

    #[test]
    fn restore_has_no_participants() {
        let mut r = room();
        host(&mut r);
        let restored = Room::restore(config(), r.persisted(0));
        assert!(restored.state().participants.is_empty());
    }
}

mod review_fixes {
    use super::*;

    #[test]
    fn player_closed_survives_item_change_and_does_not_block() {
        let mut r = room();
        let h = host(&mut r);
        let g = join(&mut r, "guest");
        add(
            &mut r,
            0,
            h,
            vec![Source::Url { url: "https://a".into() }, Source::Url { url: "https://b".into() }],
            None,
        );
        status(&mut r, 0, g, 1, MediaStatus::PlayerClosed);
        r.command(10, h, ClientMsg::PlaylistSelect { req_id: 1, item_id: ItemId(2) });
        assert_eq!(participant(&r, g).unwrap().status, MediaStatus::PlayerClosed);
        assert_eq!(r.playback().hold.as_ref().unwrap().waiting_for, vec![h]);
        status(&mut r, 20, h, 2, ready());
        assert!(r.playback().hold.is_none());
    }

    #[test]
    fn huge_seek_without_duration_does_not_overflow() {
        let mut r = room();
        let h = host(&mut r);
        add(&mut r, 0, h, vec![Source::Url { url: "https://a".into() }], None);
        status(&mut r, 0, h, 1, ready());
        r.command(0, h, ClientMsg::Play { req_id: 1, position_ms: None });
        r.command(10, h, ClientMsg::Seek { req_id: 2, position_ms: u64::MAX });
        assert!(r.playback().is_playing());
        let _ = r.playback().expected_position_ms(1_000_000);
        r.tick(1_000_000);
        let _ = r.persisted(2_000_000);
    }

    #[test]
    fn item_ended_ignored_during_hold_and_at_start() {
        let mut r = room();
        let h = host(&mut r);
        add(
            &mut r,
            0,
            h,
            vec![Source::Url { url: "https://a".into() }, Source::Url { url: "https://b".into() }],
            None,
        );
        r.command(0, h, ClientMsg::PlaylistSelect { req_id: 1, item_id: ItemId(2) });
        // Hold ItemStart, duration unknown: a stale EOF must not skip item 2.
        assert!(r.command(100, h, ClientMsg::ItemEnded { item_id: ItemId(2) }).is_empty());
        status(&mut r, 200, h, 2, ready());
        assert!(
            r.command(1_000, h, ClientMsg::ItemEnded { item_id: ItemId(2) }).is_empty(),
            "too early"
        );
        assert_eq!(r.playback().item_id, Some(ItemId(2)));
    }

    #[test]
    fn left_and_kicked_notices_come_before_the_room_update() {
        let mut r = room();
        let h = host(&mut r);
        let g = join(&mut r, "guest");
        let effects = r.leave(0, g);
        let notice = effects
            .iter()
            .position(|e| matches!(e, Effect::Broadcast { msg: ServerMsg::Event(_) }))
            .unwrap();
        let room_update = effects
            .iter()
            .position(|e| matches!(e, Effect::Broadcast { msg: ServerMsg::Room(_) }))
            .unwrap();
        assert!(notice < room_update, "{effects:?}");
        let g2 = join(&mut r, "guest2");
        let effects = r.command(0, h, ClientMsg::Kick { req_id: 1, participant_id: g2 });
        let notice = effects
            .iter()
            .position(|e| matches!(e, Effect::Broadcast { msg: ServerMsg::Event(_) }))
            .unwrap();
        let room_update = effects
            .iter()
            .position(|e| matches!(e, Effect::Broadcast { msg: ServerMsg::Room(_) }))
            .unwrap();
        assert!(notice < room_update, "{effects:?}");
    }
}

#[test]
fn playback_seq_strictly_increases_within_one_request() {
    let mut r = room();
    let h = host(&mut r);
    let effects = add(&mut r, 0, h, vec![Source::Url { url: "https://a".into() }], None);
    let seqs: Vec<u64> = effects
        .iter()
        .filter_map(|e| match e {
            Effect::Broadcast { msg: ServerMsg::Playback(p) } => Some(p.seq),
            _ => None,
        })
        .collect();
    assert!(!seqs.is_empty());
    assert!(seqs.windows(2).all(|w| w[0] < w[1]), "{seqs:?}");
}

mod v2 {
    use super::*;

    fn waiting_room() -> (Room, ParticipantId, ParticipantId) {
        let mut r = room();
        let h = host(&mut r);
        let g = join(&mut r, "guest");
        add(&mut r, 0, h, vec![Source::Url { url: "https://a".into() }], None);
        r.command(0, h, ClientMsg::Play { req_id: 1, position_ms: None });
        status(&mut r, 0, h, 1, ready());
        assert!(r.playback().hold.is_some(), "waiting for the guest");
        (r, h, g)
    }

    #[test]
    fn host_can_start_without_waiting() {
        let (mut r, h, _) = waiting_room();
        let effects = r.command(5_000, h, ClientMsg::StartWithoutWaiting { req_id: 2 });
        let p = r.playback();
        assert!(p.hold.is_none());
        assert!(p.is_playing());
        assert_eq!(p.at_server_ms, 5_000);
        assert!(has_notice(&effects, |a| *a == NoticeAction::HoldEnded));
    }

    #[test]
    fn guest_cannot_start_without_waiting() {
        let (mut r, _, g) = waiting_room();
        let effects = r.command(0, g, ClientMsg::StartWithoutWaiting { req_id: 3 });
        assert_denied(&effects, g, 3, ErrorCode::NotHost);
        assert!(r.playback().hold.is_some());
    }

    #[test]
    fn progress_carries_drift() {
        let (mut r, h, _) = waiting_room();
        r.command(
            0,
            h,
            ClientMsg::Progress {
                item_id: ItemId(1),
                delivery: Delivery::Local,
                drift_ms: Some(-42),
            },
        );
        let effects = r.tick(2_000);
        let entries = effects
            .iter()
            .find_map(|e| match e {
                Effect::Broadcast { msg: ServerMsg::ProgressBatch { entries } } => {
                    Some(entries.clone())
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(entries[0].drift_ms, Some(-42));
    }

    fn progress_batch(effects: &[Effect]) -> Option<Vec<ProgressEntry>> {
        effects.iter().find_map(|e| match e {
            Effect::Broadcast { msg: ServerMsg::ProgressBatch { entries } } => {
                Some(entries.clone())
            }
            _ => None,
        })
    }

    /// A drift measured before a pause, seek or speed change says nothing about the new
    /// position: it is forgotten until the participant measures again.
    #[test]
    fn playback_change_forgets_measured_drift() {
        let changes: [fn() -> ClientMsg; 4] = [
            || ClientMsg::Pause { req_id: 5, position_ms: None },
            || ClientMsg::Seek { req_id: 5, position_ms: 30_000 },
            || ClientMsg::SetSpeed { req_id: 5, speed: 1.5 },
            || ClientMsg::Play { req_id: 5, position_ms: Some(1_000) },
        ];
        for change in changes {
            let (mut r, h, _) = waiting_room();
            r.command(0, h, ClientMsg::StartWithoutWaiting { req_id: 2 });
            let drift = ClientMsg::Progress {
                item_id: ItemId(1),
                delivery: Delivery::Local,
                drift_ms: Some(169),
            };
            r.command(0, h, drift);
            assert_eq!(progress_batch(&r.tick(2_000)).unwrap()[0].drift_ms, Some(169));
            r.command(3_000, h, change());
            let entries = progress_batch(&r.tick(5_000)).expect("the change is broadcast");
            assert_eq!(entries[0].drift_ms, None, "after {:?}", change());
        }
    }
}

/// The server ends items itself: by the file's own duration when it knows it, otherwise
/// when every active participant reached the end (spec §6.1, "End of item").
mod item_end {
    use super::*;

    fn library_room(duration_ms: Option<u64>) -> (Room, ParticipantId, ParticipantId) {
        let mut r = room();
        let h = host(&mut r);
        let g = join(&mut r, "guest");
        let info = |id: &str| FileInfo { duration_ms, ..file_info(id) };
        r.set_library(0, vec![info("a"), info("b")]);
        add(&mut r, 0, h, vec![file_src("a"), file_src("b")], None);
        r.command(0, h, ClientMsg::PlaylistSelect { req_id: 1, item_id: ItemId(1) });
        status(&mut r, 0, h, 1, ready());
        status(&mut r, 0, g, 1, ready());
        assert!(r.playback().is_playing());
        (r, h, g)
    }

    #[test]
    fn the_files_own_duration_is_sent_and_beats_reports() {
        let (mut r, _, g) = library_room(Some(90_000));
        assert_eq!(r.state().playlist[0].duration_ms, Some(90_000));
        status(&mut r, 0, g, 1, MediaStatus::Ready { duration_ms: Some(1_000), title: None });
        assert_eq!(r.state().playlist[0].duration_ms, Some(90_000));
    }

    #[test]
    fn the_room_moves_on_at_the_end_even_with_nobody_watching() {
        let (mut r, h, g) = library_room(Some(10_000));
        status(&mut r, 100, h, 1, MediaStatus::PlayerClosed);
        r.disconnected(200, g);
        assert!(
            r.tick(9_999)
                .iter()
                .all(|e| !matches!(e, Effect::Broadcast { msg: ServerMsg::Playback(_) }))
        );
        assert_eq!(r.playback().item_id, Some(ItemId(1)));
        let effects = r.tick(10_000);
        assert_eq!(last_playback(&effects).item_id, Some(ItemId(2)));
    }

    #[test]
    fn players_cannot_end_an_item_whose_duration_the_server_knows() {
        let (mut r, h, g) = library_room(Some(60_000));
        for pid in [h, g] {
            assert!(r.command(20_000, pid, ClientMsg::ItemEnded { item_id: ItemId(1) }).is_empty());
        }
        assert_eq!(r.playback().item_id, Some(ItemId(1)));
        r.tick(60_000);
        assert_eq!(r.playback().item_id, Some(ItemId(2)));
    }

    #[test]
    fn a_paused_or_waiting_room_does_not_move_on() {
        let (mut r, h, _) = library_room(Some(10_000));
        r.command(5_000, h, ClientMsg::Pause { req_id: 2, position_ms: None });
        r.tick(60_000);
        assert_eq!(r.playback().item_id, Some(ItemId(1)));
        r.test_set_hold(Some(Hold { reason: HoldReason::Stalled, waiting_for: vec![h] }));
        r.command(60_000, h, ClientMsg::Play { req_id: 3, position_ms: None });
        r.tick(120_000);
        assert_eq!(r.playback().item_id, Some(ItemId(1)));
    }

    #[test]
    fn the_last_item_stops_at_its_end() {
        let mut r = room();
        let h = host(&mut r);
        r.set_library(0, vec![FileInfo { duration_ms: Some(10_000), ..file_info("a") }]);
        add(&mut r, 0, h, vec![file_src("a")], None);
        r.command(0, h, ClientMsg::PlaylistSelect { req_id: 1, item_id: ItemId(1) });
        status(&mut r, 0, h, 1, ready());
        let p = last_playback(&r.tick(10_500));
        assert_eq!((p.item_id, p.paused, p.position_ms), (Some(ItemId(1)), true, 10_000));
        assert!(r.tick(11_000).is_empty(), "stays there");
    }

    #[test]
    fn one_participant_alone_cannot_end_a_link() {
        let mut r = room();
        let h = host(&mut r);
        let g = join(&mut r, "guest");
        add(
            &mut r,
            0,
            h,
            vec![Source::Url { url: "https://a".into() }, Source::Url { url: "https://b".into() }],
            None,
        );
        r.command(0, h, ClientMsg::PlaylistSelect { req_id: 1, item_id: ItemId(1) });
        status(&mut r, 0, h, 1, ready());
        // The guest lies first: a 1 ms duration, then "ended".
        status(&mut r, 0, g, 1, MediaStatus::Ready { duration_ms: Some(1), title: None });
        r.tick(5_000);
        assert!(r.command(5_000, g, ClientMsg::ItemEnded { item_id: ItemId(1) }).is_empty());
        assert_eq!(r.playback().item_id, Some(ItemId(1)), "the guest cannot skip");
        status(&mut r, 5_000, h, 1, MediaStatus::Ready { duration_ms: Some(30_000), title: None });
        r.command(30_000, h, ClientMsg::ItemEnded { item_id: ItemId(1) });
        assert_eq!(r.playback().item_id, Some(ItemId(2)), "everyone has reached the end");
    }

    #[test]
    fn a_link_moves_on_late_if_a_player_never_reports_its_end() {
        let mut r = room();
        let h = host(&mut r);
        let g = join(&mut r, "guest");
        add(
            &mut r,
            0,
            h,
            vec![Source::Url { url: "https://a".into() }, Source::Url { url: "https://b".into() }],
            None,
        );
        r.command(0, h, ClientMsg::PlaylistSelect { req_id: 1, item_id: ItemId(1) });
        status(&mut r, 0, h, 1, MediaStatus::Ready { duration_ms: Some(30_000), title: None });
        status(&mut r, 0, g, 1, ready());
        r.command(30_000, h, ClientMsg::ItemEnded { item_id: ItemId(1) });
        r.tick(39_000);
        assert_eq!(r.playback().item_id, Some(ItemId(1)), "the guest may still be playing");
        r.tick(40_000);
        assert_eq!(r.playback().item_id, Some(ItemId(2)), "10 s after the reported end");
    }

    #[test]
    fn a_clip_shorter_than_the_start_guard_still_ends() {
        let mut r = room();
        let h = host(&mut r);
        add(
            &mut r,
            0,
            h,
            vec![Source::Url { url: "https://a".into() }, Source::Url { url: "https://b".into() }],
            None,
        );
        r.command(0, h, ClientMsg::PlaylistSelect { req_id: 1, item_id: ItemId(1) });
        status(&mut r, 0, h, 1, MediaStatus::Ready { duration_ms: Some(2_000), title: None });
        assert!(
            r.command(500, h, ClientMsg::ItemEnded { item_id: ItemId(1) }).is_empty(),
            "a stale EOF"
        );
        r.command(2_000, h, ClientMsg::ItemEnded { item_id: ItemId(1) });
        assert_eq!(r.playback().item_id, Some(ItemId(2)));
    }

    #[test]
    fn participants_who_left_do_not_hold_up_the_end() {
        let mut r = room();
        let h = host(&mut r);
        let g = join(&mut r, "guest");
        add(
            &mut r,
            0,
            h,
            vec![Source::Url { url: "https://a".into() }, Source::Url { url: "https://b".into() }],
            None,
        );
        r.command(0, h, ClientMsg::PlaylistSelect { req_id: 1, item_id: ItemId(1) });
        status(&mut r, 0, h, 1, ready());
        status(&mut r, 0, g, 1, ready());
        status(&mut r, 1_000, h, 1, MediaStatus::PlayerClosed);
        r.command(20_000, g, ClientMsg::ItemEnded { item_id: ItemId(1) });
        assert_eq!(r.playback().item_id, Some(ItemId(2)));
    }
}

/// Whoever has the invite link must not be able to grow the server's state or flood the
/// others without bound.
mod limits {
    use super::*;

    fn urls(n: usize) -> Vec<Source> {
        (0..n).map(|i| Source::Url { url: format!("https://example.org/{i}") }).collect()
    }

    fn denied_code(effects: &[Effect]) -> Option<ErrorCode> {
        effects.iter().find_map(|e| match e {
            Effect::Send { msg: ServerMsg::Denied { code, .. }, .. } => Some(code.clone()),
            _ => None,
        })
    }

    #[test]
    fn playlist_stops_growing_at_its_limit() {
        let mut r = room();
        let h = host(&mut r);
        assert_eq!(denied_code(&add(&mut r, 0, h, urls(MAX_PLAYLIST_ITEMS - 1), None)), None);
        let full = ErrorCode::PlaylistFull { max_items: MAX_PLAYLIST_ITEMS as u32 };
        assert_eq!(denied_code(&add(&mut r, 0, h, urls(2), None)), Some(full.clone()));
        assert_eq!(r.state().playlist.len(), MAX_PLAYLIST_ITEMS - 1, "all or nothing");
        assert_eq!(denied_code(&add(&mut r, 0, h, urls(1), None)), None);
        assert_eq!(denied_code(&add(&mut r, 0, h, urls(1), None)), Some(full));
        assert_eq!(r.state().playlist.len(), MAX_PLAYLIST_ITEMS);
    }

    #[test]
    fn overlong_url_is_refused() {
        let mut r = room();
        let h = host(&mut r);
        let long = format!("https://example.org/{}", "a".repeat(MAX_URL_CHARS));
        let effects = add(&mut r, 0, h, vec![Source::Url { url: long }], None);
        assert_eq!(denied_code(&effects), Some(ErrorCode::InvalidRequest));
        assert!(r.state().playlist.is_empty());
    }

    fn chat(r: &mut Room, now: u64, pid: ParticipantId) -> bool {
        r.command(now, pid, ClientMsg::Chat { text: "hi".into() })
            .iter()
            .any(|e| matches!(e, Effect::Broadcast { msg: ServerMsg::Event(Event::Chat(_)) }))
    }

    #[test]
    fn chat_allows_a_burst_then_one_message_a_second() {
        let mut r = room();
        let h = host(&mut r);
        let g = join(&mut r, "guest");
        assert!((0..CHAT_BURST).all(|_| chat(&mut r, 0, g)));
        assert!(!chat(&mut r, 500, g), "flood dropped");
        assert!(chat(&mut r, 0, h), "others can still talk");
        assert!(chat(&mut r, CHAT_REFILL_MS, g));
        assert!(!chat(&mut r, CHAT_REFILL_MS, g));
    }
}

/// What the room does about a participant who falls behind.
mod lag_policy {
    use super::*;

    /// Host and guest ready; the room plays from 0 at time 0.
    fn playing_room() -> (Room, ParticipantId, ParticipantId) {
        let mut r = room();
        let h = host(&mut r);
        let g = join(&mut r, "guest");
        add(&mut r, 0, h, vec![Source::Url { url: "https://a".into() }], None);
        r.command(0, h, ClientMsg::Play { req_id: 1, position_ms: None });
        status(&mut r, 0, h, 1, ready());
        status(&mut r, 0, g, 1, ready());
        assert!(r.playback().is_playing(), "{:?}", r.playback());
        (r, h, g)
    }

    fn drift(r: &mut Room, now: u64, pid: ParticipantId, ms: i64) {
        let msg = ClientMsg::Progress {
            item_id: ItemId(1),
            delivery: Delivery::Stream,
            drift_ms: Some(ms),
        };
        r.command(now, pid, msg);
    }

    fn follow_slowest(r: &mut Room, h: ParticipantId) {
        r.command(0, h, ClientMsg::SetLagPolicy { req_id: 7, policy: LagPolicy::FollowSlowest });
    }

    #[test]
    fn only_the_host_sets_it_and_everyone_hears() {
        let (mut r, h, g) = playing_room();
        let msg = |req_id| ClientMsg::SetLagPolicy { req_id, policy: LagPolicy::FollowSlowest };
        assert_denied(&r.command(0, g, msg(5)), g, 5, ErrorCode::NotHost);
        assert_eq!(r.state().lag_policy, LagPolicy::CatchUpAlone);
        let effects = r.command(0, h, msg(6));
        assert_eq!(r.state().lag_policy, LagPolicy::FollowSlowest);
        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::Broadcast { msg: ServerMsg::Room(s) } if s.lag_policy == LagPolicy::FollowSlowest
        )));
        assert!(has_notice(&effects, |a| *a
            == NoticeAction::LagPolicyChanged { policy: LagPolicy::FollowSlowest }));
        assert!(effects.contains(&Effect::Persist));
    }

    #[test]
    fn by_default_the_room_plays_on() {
        let (mut r, _, g) = playing_room();
        drift(&mut r, 10_000, g, -5_000);
        r.tick(10_000);
        assert_eq!(r.playback().expected_position_ms(10_000), 10_000);
    }

    #[test]
    fn the_room_goes_back_to_the_slowest() {
        let (mut r, h, g) = playing_room();
        follow_slowest(&mut r, h);
        drift(&mut r, 10_000, h, -50);
        drift(&mut r, 10_000, g, -1_500);
        let effects = r.tick(10_000);
        assert_eq!(r.playback().expected_position_ms(10_000), 8_500);
        assert!(
            effects.iter().any(|e| matches!(e, Effect::Broadcast { msg: ServerMsg::Playback(_) }))
        );
        assert!(has_notice(&effects, |a| *a
            == NoticeAction::WentBackFor { participant_id: g, back_ms: 1_500 }));
        // Measured against the old position: not used twice.
        r.tick(10_250);
        assert_eq!(r.playback().expected_position_ms(10_250), 8_750);
    }

    #[test]
    fn a_small_lag_is_left_to_the_participant() {
        let (mut r, h, g) = playing_room();
        follow_slowest(&mut r, h);
        drift(&mut r, 10_000, g, -(FOLLOW_SLOWEST_MS as i64) + 1);
        r.tick(10_000);
        assert_eq!(r.playback().expected_position_ms(10_000), 10_000);
    }

    #[test]
    fn a_buffering_or_absent_participant_does_not_pull_the_room_back() {
        let (mut r, h, g) = playing_room();
        follow_slowest(&mut r, h);
        drift(&mut r, 10_000, g, -5_000);
        // Long stalls are what the stall hold is for.
        status(&mut r, 10_000, g, 1, MediaStatus::Stalled);
        r.tick(10_000);
        assert_eq!(r.playback().expected_position_ms(10_000), 10_000);
        status(&mut r, 10_000, g, 1, ready());
        drift(&mut r, 10_000, g, -5_000);
        r.disconnected(10_000, g);
        r.tick(10_000);
        assert_eq!(r.playback().expected_position_ms(10_000), 10_000);
    }

    #[test]
    fn a_paused_room_stays_where_it_is() {
        let (mut r, h, g) = playing_room();
        follow_slowest(&mut r, h);
        drift(&mut r, 10_000, g, -2_000);
        r.command(10_000, h, ClientMsg::Pause { req_id: 9, position_ms: Some(10_000) });
        drift(&mut r, 10_000, g, -2_000);
        r.tick(10_000);
        assert_eq!(r.playback().position_ms, 10_000);
    }

    #[test]
    fn the_policy_survives_a_restart() {
        let (mut r, h, _) = playing_room();
        follow_slowest(&mut r, h);
        let restored = Room::restore(config(), r.persisted(1_000));
        assert_eq!(restored.state().lag_policy, LagPolicy::FollowSlowest);
    }
}

mod ready_check {
    use super::*;

    fn ready_check(r: &mut Room, h: ParticipantId, on: bool) -> Vec<Effect> {
        r.command(0, h, ClientMsg::SetReadyCheck { req_id: 7, on })
    }

    fn press_ready(r: &mut Room, now: u64, pid: ParticipantId, ready: bool) -> Vec<Effect> {
        r.command(now, pid, ClientMsg::SetReady { ready })
    }

    /// Host and guest in a room with the check on; item 1 just started (waiting).
    fn starting_room() -> (Room, ParticipantId, ParticipantId) {
        let mut r = room();
        let h = host(&mut r);
        let g = join(&mut r, "guest");
        ready_check(&mut r, h, true);
        add(&mut r, 0, h, vec![Source::Url { url: "https://a".into() }], None);
        r.command(0, h, ClientMsg::Play { req_id: 1, position_ms: None });
        (r, h, g)
    }

    fn is_ready(r: &Room, pid: ParticipantId) -> bool {
        participant(r, pid).unwrap().ready
    }

    #[test]
    fn only_the_host_turns_it_on_and_everyone_hears() {
        let mut r = room();
        let h = host(&mut r);
        let g = join(&mut r, "guest");
        let msg = |req_id| ClientMsg::SetReadyCheck { req_id, on: true };
        assert_denied(&r.command(0, g, msg(5)), g, 5, ErrorCode::NotHost);
        assert!(!r.state().ready_check, "off by default");
        let effects = r.command(0, h, msg(6));
        assert!(r.state().ready_check);
        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::Broadcast { msg: ServerMsg::Room(s) } if s.ready_check
        )));
        assert!(has_notice(&effects, |a| *a == NoticeAction::ReadyCheckChanged { on: true }));
        assert!(effects.contains(&Effect::Persist));
    }

    #[test]
    fn the_start_waits_for_everyone_to_press_ready() {
        let (mut r, h, g) = starting_room();
        status(&mut r, 0, h, 1, ready());
        status(&mut r, 0, g, 1, ready());
        assert!(r.playback().hold.is_some(), "loaded is not enough");
        let effects = press_ready(&mut r, 100, h, true);
        assert!(is_ready(&r, h));
        assert!(has_notice(&effects, |a| *a == NoticeAction::ReadyChanged { ready: true }));
        assert!(r.playback().hold.is_some(), "the guest has not pressed it");
        press_ready(&mut r, 200, g, true);
        assert!(r.playback().is_playing(), "{:?}", r.playback());
    }

    #[test]
    fn pressing_ready_before_the_file_loaded_still_waits_for_the_file() {
        let (mut r, h, g) = starting_room();
        press_ready(&mut r, 0, h, true);
        press_ready(&mut r, 0, g, true);
        status(&mut r, 0, h, 1, ready());
        assert!(r.playback().hold.is_some());
        status(&mut r, 0, g, 1, ready());
        assert!(r.playback().is_playing());
    }

    #[test]
    fn taking_ready_back_keeps_the_room_waiting() {
        let (mut r, h, g) = starting_room();
        status(&mut r, 0, h, 1, ready());
        status(&mut r, 0, g, 1, ready());
        press_ready(&mut r, 0, h, true);
        press_ready(&mut r, 0, h, false);
        press_ready(&mut r, 0, g, true);
        assert!(r.playback().hold.is_some());
        assert!(!is_ready(&r, h));
    }

    #[test]
    fn ready_is_reset_for_every_new_item() {
        let (mut r, h, g) = starting_room();
        add(&mut r, 0, h, vec![Source::Url { url: "https://b".into() }], None);
        for p in [h, g] {
            status(&mut r, 0, p, 1, ready());
            press_ready(&mut r, 0, p, true);
        }
        assert!(r.playback().is_playing());
        r.command(1_000, h, ClientMsg::PlaylistSelect { req_id: 3, item_id: ItemId(2) });
        assert!(!is_ready(&r, h) && !is_ready(&r, g));
        for p in [h, g] {
            status(&mut r, 1_000, p, 2, ready());
        }
        assert!(r.playback().hold.is_some(), "waits for Ready again");
    }

    #[test]
    fn turning_it_off_releases_a_room_waiting_only_for_ready() {
        let (mut r, h, g) = starting_room();
        status(&mut r, 0, h, 1, ready());
        status(&mut r, 0, g, 1, ready());
        assert!(r.playback().hold.is_some());
        ready_check(&mut r, h, false);
        assert!(r.playback().is_playing());
    }

    #[test]
    fn without_the_check_ready_changes_nothing() {
        let mut r = room();
        let h = host(&mut r);
        let g = join(&mut r, "guest");
        add(&mut r, 0, h, vec![Source::Url { url: "https://a".into() }], None);
        r.command(0, h, ClientMsg::Play { req_id: 1, position_ms: None });
        status(&mut r, 0, h, 1, ready());
        status(&mut r, 0, g, 1, ready());
        assert!(r.playback().is_playing());
    }

    #[test]
    fn a_stall_hold_does_not_wait_for_ready() {
        let (mut r, h, g) = starting_room();
        for p in [h, g] {
            status(&mut r, 0, p, 1, ready());
            press_ready(&mut r, 0, p, true);
        }
        status(&mut r, 1_000, g, 1, MediaStatus::Stalled);
        r.tick(11_000);
        assert!(r.playback().hold.is_some(), "stall hold");
        press_ready(&mut r, 11_000, g, false);
        status(&mut r, 11_500, g, 1, ready());
        assert!(r.playback().hold.is_none(), "{:?}", r.playback());
    }

    #[test]
    fn the_same_ready_twice_tells_nobody() {
        let (mut r, h, _) = starting_room();
        press_ready(&mut r, 0, h, true);
        assert!(press_ready(&mut r, 0, h, true).is_empty());
    }

    #[test]
    fn the_check_survives_a_restart() {
        let (r, _, _) = starting_room();
        let restored = Room::restore(config(), r.persisted(1_000));
        assert!(restored.state().ready_check);
    }
}
