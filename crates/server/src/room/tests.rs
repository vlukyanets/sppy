use super::*;

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
