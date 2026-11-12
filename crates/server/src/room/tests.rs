use super::*;
use sppy_proto::*;

pub(super) fn config() -> RoomConfig {
    RoomConfig {
        max_participants: 8,
        resume_grace_ms: 60_000,
        settings: RoomSettings {
            stall_hold_secs: 10,
            resume_buffer_secs: 10,
        },
    }
}

pub(super) fn room() -> Room {
    Room::new(config())
}

pub(super) fn join_as(room: &mut Room, now: u64, nick: &str, role: JoinRole) -> ParticipantId {
    room.join(
        now,
        JoinRequest {
            nickname: nick.into(),
            role,
            session_token: format!("sess-{nick}"),
        },
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
            Effect::Send {
                to,
                msg: ServerMsg::Welcome(w),
            } => {
                assert_eq!(*to, ParticipantId(1));
                assert_eq!(w.participant_id, ParticipantId(1));
                assert!(!w.is_host);
                assert_eq!(w.session_token, "s");
                assert_eq!(w.server_time_ms, 5);
                assert_eq!(w.room.participants.len(), 1);
            }
            other => panic!("expected Welcome, got {other:?}"),
        }
        assert!(matches!(
            joined.effects[1],
            Effect::Broadcast {
                msg: ServerMsg::Room(_)
            }
        ));
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
                    JoinRequest {
                        nickname: bad,
                        role: JoinRole::Guest,
                        session_token: "x".into(),
                    },
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
        assert!(matches!(
            &joined.effects[0],
            Effect::Send {
                msg: ServerMsg::Welcome(_),
                ..
            }
        ));
    }

    #[test]
    fn resume_after_grace_fails() {
        let mut r = room();
        let a = join(&mut r, "alice");
        r.disconnected(0, a);
        r.tick(60_000);
        assert_eq!(
            r.resume(60_001, "sess-alice").unwrap_err(),
            ErrorCode::SessionExpired
        );
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
                msg: ServerMsg::Event(Event::Notice {
                    action: NoticeAction::Left,
                    ..
                })
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
        let Effect::Send {
            msg: ServerMsg::Welcome(w),
            ..
        } = &joined.effects[0]
        else {
            panic!()
        };
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
    assert_eq!(
        effects[0],
        Effect::Send {
            to,
            msg: ServerMsg::Denied { req_id, code }
        }
    );
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
            ClientMsg::SetCanControl {
                req_id: 2,
                participant_id: g,
                allowed: true,
            },
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
        let effects = r.command(
            0,
            h,
            ClientMsg::Kick {
                req_id: 3,
                participant_id: g,
            },
        );
        assert!(effects.contains(&Effect::Close {
            to: g,
            reason: CloseReason::Kicked
        }));
        assert!(participant(&r, g).is_none());
        assert!(!r.session_valid("sess-guest"));
        assert!(has_notice(&effects, |a| *a
            == NoticeAction::Kicked { participant_id: g }));
    }

    #[test]
    fn kick_by_guest_denied() {
        let mut r = room();
        let h = host(&mut r);
        let g = join(&mut r, "guest");
        let effects = r.command(
            0,
            g,
            ClientMsg::Kick {
                req_id: 5,
                participant_id: h,
            },
        );
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
            == NoticeAction::ControlModeChanged {
                mode: ControlMode::HostOnly
            }));
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
        source: Source::Url {
            url: format!("https://example.com/{id}"),
        },
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
            Effect::Broadcast {
                msg: ServerMsg::Playback(p),
            } => Some(p.clone()),
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
        r.command(
            1000,
            h,
            ClientMsg::Play {
                req_id: 1,
                position_ms: None,
            },
        );
        let effects = r.command(
            4000,
            h,
            ClientMsg::Pause {
                req_id: 2,
                position_ms: None,
            },
        );
        let p = last_playback(&effects);
        assert_eq!(p.position_ms, 3000);
        assert_eq!(p.at_server_ms, 4000);
        assert!(p.paused);
        assert_eq!(p.seq, 2);
        assert_eq!(
            p.cause,
            Some(Cause {
                participant_id: h,
                req_id: 2
            })
        );
        assert!(has_notice(&effects, |a| *a == NoticeAction::Paused));
        assert!(effects.contains(&Effect::Persist));
    }

    #[test]
    fn pause_uses_client_position() {
        let (mut r, h, _) = setup(None);
        r.command(
            1000,
            h,
            ClientMsg::Play {
                req_id: 1,
                position_ms: None,
            },
        );
        let effects = r.command(
            4000,
            h,
            ClientMsg::Pause {
                req_id: 2,
                position_ms: Some(2950),
            },
        );
        assert_eq!(last_playback(&effects).position_ms, 2950);
    }

    #[test]
    fn play_notice_and_state() {
        let (mut r, h, _) = setup(None);
        let effects = r.command(
            1000,
            h,
            ClientMsg::Play {
                req_id: 1,
                position_ms: Some(500),
            },
        );
        let p = last_playback(&effects);
        assert!(!p.paused);
        assert_eq!((p.position_ms, p.at_server_ms), (500, 1000));
        assert!(has_notice(&effects, |a| *a == NoticeAction::Resumed));
    }

    #[test]
    fn seek_while_playing() {
        let (mut r, h, _) = setup(None);
        r.command(
            1000,
            h,
            ClientMsg::Play {
                req_id: 1,
                position_ms: None,
            },
        );
        let effects = r.command(
            5000,
            h,
            ClientMsg::Seek {
                req_id: 2,
                position_ms: 30_000,
            },
        );
        let p = last_playback(&effects);
        assert_eq!(
            (p.position_ms, p.at_server_ms, p.paused),
            (30_000, 5000, false)
        );
        assert!(has_notice(&effects, |a| *a == NoticeAction::Seeked { to_ms: 30_000 }));
    }

    #[test]
    fn seek_beyond_duration_clamps() {
        let (mut r, h, _) = setup(Some(60_000));
        let effects = r.command(
            0,
            h,
            ClientMsg::Seek {
                req_id: 1,
                position_ms: 90_000,
            },
        );
        assert_eq!(last_playback(&effects).position_ms, 60_000);
    }

    #[test]
    fn set_speed() {
        let (mut r, h, _) = setup(None);
        r.command(
            0,
            h,
            ClientMsg::Play {
                req_id: 1,
                position_ms: None,
            },
        );
        let effects = r.command(
            2000,
            h,
            ClientMsg::SetSpeed {
                req_id: 2,
                speed: 1.5,
            },
        );
        let p = last_playback(&effects);
        assert_eq!((p.speed, p.position_ms, p.at_server_ms), (1.5, 2000, 2000));
        assert!(has_notice(&effects, |a| *a
            == NoticeAction::SpeedChanged { speed: 1.5 }));
    }

    #[test]
    fn speed_out_of_range_denied() {
        let (mut r, h, _) = setup(None);
        for (i, speed) in [0.1, 5.0, f64::NAN].into_iter().enumerate() {
            let effects = r.command(
                0,
                h,
                ClientMsg::SetSpeed {
                    req_id: i as u32,
                    speed,
                },
            );
            assert_denied(&effects, h, i as u32, ErrorCode::InvalidRequest);
        }
    }

    #[test]
    fn no_current_item_denied() {
        let mut r = room();
        let h = host(&mut r);
        let effects = r.command(
            0,
            h,
            ClientMsg::Play {
                req_id: 1,
                position_ms: None,
            },
        );
        assert_denied(&effects, h, 1, ErrorCode::InvalidRequest);
    }

    #[test]
    fn guest_without_rights_denied_gets_state() {
        let (mut r, h, g) = setup(None);
        r.command(
            0,
            h,
            ClientMsg::SetControlMode {
                req_id: 1,
                mode: ControlMode::HostOnly,
            },
        );
        let effects = r.command(
            0,
            g,
            ClientMsg::Pause {
                req_id: 7,
                position_ms: None,
            },
        );
        assert_denied(&effects, g, 7, ErrorCode::NotPermitted);
    }

    #[test]
    fn pause_during_hold_kept_after_hold() {
        let (mut r, h, _) = setup(None);
        r.command(
            0,
            h,
            ClientMsg::Play {
                req_id: 1,
                position_ms: None,
            },
        );
        r.test_set_hold(Some(Hold {
            reason: HoldReason::ItemStart,
            waiting_for: vec![h],
        }));
        r.command(
            100,
            h,
            ClientMsg::Pause {
                req_id: 2,
                position_ms: None,
            },
        );
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
        subtitles: vec![],
    }
}

pub(super) fn file_src(id: &str) -> Source {
    Source::File {
        file_id: FileId(id.into()),
    }
}

pub(super) fn add(
    r: &mut Room,
    now: u64,
    pid: ParticipantId,
    sources: Vec<Source>,
    after: Option<ItemId>,
) -> Vec<Effect> {
    r.command(
        now,
        pid,
        ClientMsg::PlaylistAdd {
            req_id: 100,
            sources,
            after,
        },
    )
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
            ClientMsg::PlaylistAdd {
                req_id: 1,
                sources: vec![file_src("zzz")],
                after: None,
            },
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
            == NoticeAction::PlaylistAdded {
                item_ids: vec![ItemId(1)]
            }));
        assert!(effects.contains(&Effect::Persist));
    }

    #[test]
    fn add_url_uses_url_as_title() {
        let (mut r, h) = setup();
        add(
            &mut r,
            0,
            h,
            vec![Source::Url {
                url: "https://x/y".into(),
            }],
            None,
        );
        assert_eq!(r.state().playlist[0].title, "https://x/y");
    }

    #[test]
    fn add_after_and_at_start() {
        let (mut r, h) = setup();
        add(&mut r, 0, h, vec![file_src("a"), file_src("b")], None); // [1, 2]
        add(&mut r, 0, h, vec![file_src("c")], Some(ItemId(1))); // [1, 3, 2]
        assert_eq!(playlist_ids(&r), vec![1, 3, 2]);
        add(&mut r, 0, h, vec![file_src("c")], None); // at start
        assert_eq!(playlist_ids(&r), vec![4, 1, 3, 2]);
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
        add(
            &mut r,
            0,
            h,
            vec![file_src("a"), file_src("b"), file_src("c")],
            None,
        ); // [1,2,3]
        let effects = r.command(
            0,
            h,
            ClientMsg::PlaylistMove {
                req_id: 1,
                item_id: ItemId(3),
                after: None,
            },
        );
        assert_eq!(playlist_ids(&r), vec![3, 1, 2]);
        assert!(has_notice(&effects, |a| *a
            == NoticeAction::PlaylistMoved { item_id: ItemId(3) }));
        r.command(
            0,
            h,
            ClientMsg::PlaylistMove {
                req_id: 2,
                item_id: ItemId(3),
                after: Some(ItemId(2)),
            },
        );
        assert_eq!(playlist_ids(&r), vec![1, 2, 3]);
        let effects = r.command(
            0,
            h,
            ClientMsg::PlaylistMove {
                req_id: 3,
                item_id: ItemId(2),
                after: Some(ItemId(2)),
            },
        );
        assert_denied(&effects, h, 3, ErrorCode::InvalidRequest);
    }

    #[test]
    fn remove_non_current() {
        let (mut r, h) = setup();
        add(&mut r, 0, h, vec![file_src("a"), file_src("b")], None);
        let effects = r.command(
            0,
            h,
            ClientMsg::PlaylistRemove {
                req_id: 1,
                item_id: ItemId(2),
            },
        );
        assert_eq!(playlist_ids(&r), vec![1]);
        assert_eq!(r.playback().item_id, Some(ItemId(1)));
        assert!(has_notice(&effects, |a| *a
            == NoticeAction::PlaylistRemoved { item_id: ItemId(2) }));
        let effects = r.command(
            0,
            h,
            ClientMsg::PlaylistRemove {
                req_id: 2,
                item_id: ItemId(2),
            },
        );
        assert_denied(&effects, h, 2, ErrorCode::UnknownItem);
    }

    #[test]
    fn remove_current_advances() {
        let (mut r, h) = setup();
        add(&mut r, 0, h, vec![file_src("a"), file_src("b")], None);
        let effects = r.command(
            500,
            h,
            ClientMsg::PlaylistRemove {
                req_id: 1,
                item_id: ItemId(1),
            },
        );
        let p = r.playback();
        assert_eq!(p.item_id, Some(ItemId(2)));
        assert_eq!(p.position_ms, 0);
        assert!(!p.paused);
        assert_eq!(p.hold.as_ref().unwrap().reason, HoldReason::ItemStart);
        assert!(has_notice(&effects, |a| *a
            == NoticeAction::ItemChanged { item_id: ItemId(2) }));
        r.command(
            600,
            h,
            ClientMsg::PlaylistRemove {
                req_id: 2,
                item_id: ItemId(2),
            },
        );
        assert_eq!(r.playback().item_id, None);
    }

    #[test]
    fn select_resets_statuses_and_holds() {
        let (mut r, h) = setup();
        let g = join(&mut r, "guest");
        let off = join(&mut r, "offline");
        r.disconnected(0, off);
        add(&mut r, 0, h, vec![file_src("a"), file_src("b")], None);
        r.test_set_status(
            h,
            MediaStatus::Ready {
                duration_ms: None,
                title: None,
            },
        );
        let effects = r.command(
            1000,
            h,
            ClientMsg::PlaylistSelect {
                req_id: 1,
                item_id: ItemId(2),
            },
        );
        let p = last_playback(&effects);
        assert_eq!(p.item_id, Some(ItemId(2)));
        assert!(!p.paused);
        assert_eq!((p.position_ms, p.at_server_ms), (0, 1000));
        let hold = p.hold.unwrap();
        assert_eq!(hold.reason, HoldReason::ItemStart);
        assert_eq!(hold.waiting_for, vec![h, g]);
        assert!(
            r.state()
                .participants
                .iter()
                .all(|p| p.status == MediaStatus::Loading)
        );
        let effects = r.command(
            0,
            h,
            ClientMsg::PlaylistSelect {
                req_id: 2,
                item_id: ItemId(9),
            },
        );
        assert_denied(&effects, h, 2, ErrorCode::UnknownItem);
    }

    #[test]
    fn missing_file_skipped_on_change() {
        let (mut r, h) = setup();
        add(
            &mut r,
            0,
            h,
            vec![file_src("a"), file_src("b"), file_src("c")],
            None,
        );
        r.set_library(0, vec![file_info("a"), file_info("c")]);
        let effects = r.command(
            0,
            h,
            ClientMsg::PlaylistSelect {
                req_id: 1,
                item_id: ItemId(2),
            },
        );
        assert_eq!(r.playback().item_id, Some(ItemId(3)));
        assert!(has_notice(&effects, |a| *a
            == NoticeAction::ItemSkipped {
                item_id: ItemId(2),
                reason: SkipReason::FileMissing
            }));
    }

    #[test]
    fn playlist_requires_control() {
        let (mut r, h) = setup();
        let g = join(&mut r, "guest");
        r.command(
            0,
            h,
            ClientMsg::SetControlMode {
                req_id: 1,
                mode: ControlMode::HostOnly,
            },
        );
        let effects = r.command(
            0,
            g,
            ClientMsg::PlaylistAdd {
                req_id: 2,
                sources: vec![file_src("a")],
                after: None,
            },
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
        info.fingerprint = Some(Fingerprint {
            size: 5,
            hash: "00".repeat(32),
        });
        let effects = r.set_library(0, vec![info]);
        assert_eq!(r.state().playlist[0].fingerprint.as_ref().unwrap().size, 5);
        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::Broadcast {
                msg: ServerMsg::Room(_)
            }
        )));
    }
}
