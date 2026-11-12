use sppy_sync::{Classified, EchoFilter, PendingOp, RawEvent, UserAction};

fn filter() -> EchoFilter {
    EchoFilter::new(1000)
}

#[test]
fn own_pause_is_ours() {
    let mut f = filter();
    f.issued(0, PendingOp::Pause(true));
    assert_eq!(f.observe(10, RawEvent::Pause(true), 5000), Classified::Ours);
    // Consumed: a second identical event is the user's.
    assert_eq!(
        f.observe(20, RawEvent::Pause(true), 5000),
        Classified::User(UserAction::Pause { position_ms: 5000 })
    );
}

#[test]
fn user_pause_detected() {
    let mut f = filter();
    assert_eq!(
        f.observe(0, RawEvent::Pause(true), 1234),
        Classified::User(UserAction::Pause { position_ms: 1234 })
    );
    assert_eq!(
        f.observe(0, RawEvent::Pause(false), 1234),
        Classified::User(UserAction::Play { position_ms: 1234 })
    );
}

#[test]
fn mismatched_pause_value_is_user() {
    let mut f = filter();
    f.issued(0, PendingOp::Pause(true));
    assert_eq!(
        f.observe(10, RawEvent::Pause(false), 0),
        Classified::User(UserAction::Play { position_ms: 0 })
    );
}

#[test]
fn own_seek_is_ours() {
    let mut f = filter();
    f.issued(0, PendingOp::Seek { target_ms: 9000 });
    assert_eq!(f.observe(300, RawEvent::Seeked { position_ms: 9040 }, 9040), Classified::Ours);
}

#[test]
fn user_seek_detected() {
    let mut f = filter();
    assert_eq!(
        f.observe(0, RawEvent::Seeked { position_ms: 42_000 }, 42_000),
        Classified::User(UserAction::Seek { position_ms: 42_000 })
    );
}

#[test]
fn pending_expires_after_timeout() {
    let mut f = filter();
    f.issued(0, PendingOp::Pause(true));
    assert_eq!(
        f.observe(1001, RawEvent::Pause(true), 0),
        Classified::User(UserAction::Pause { position_ms: 0 })
    );
}

#[test]
fn load_ignores_everything_until_finished() {
    let mut f = filter();
    f.issued(0, PendingOp::Load);
    assert_eq!(f.observe(5000, RawEvent::Pause(true), 0), Classified::Ignored);
    assert_eq!(f.observe(5000, RawEvent::Seeked { position_ms: 0 }, 0), Classified::Ignored);
    f.load_finished();
    assert!(matches!(f.observe(5001, RawEvent::Pause(false), 0), Classified::User(_)));
}

#[test]
fn own_speed_is_ours_foreign_speed_reverted() {
    let mut f = filter();
    f.issued(0, PendingOp::Speed(0.95));
    assert_eq!(f.observe(10, RawEvent::Speed(0.95), 0), Classified::Ours);
    assert_eq!(f.observe(20, RawEvent::Speed(1.25), 0), Classified::RevertSpeed);
}

#[test]
fn several_pending_ops_matched_independently() {
    let mut f = filter();
    f.issued(0, PendingOp::Seek { target_ms: 1 });
    f.issued(0, PendingOp::Pause(false));
    assert_eq!(f.observe(5, RawEvent::Pause(false), 0), Classified::Ours);
    assert_eq!(f.observe(200, RawEvent::Seeked { position_ms: 1 }, 1), Classified::Ours);
}

#[test]
fn user_seek_elsewhere_while_own_seek_pending() {
    let mut f = filter();
    f.issued(0, PendingOp::Seek { target_ms: 10_000 });
    // The user's seek to 5 min lands first: not ours.
    assert_eq!(
        f.observe(50, RawEvent::Seeked { position_ms: 300_000 }, 300_000),
        Classified::User(UserAction::Seek { position_ms: 300_000 })
    );
    // Our own seek lands afterwards and is still recognized.
    assert_eq!(f.observe(150, RawEvent::Seeked { position_ms: 10_010 }, 10_010), Classified::Ours);
}

#[test]
fn seek_pending_reports_own_seeks_only_until_timeout() {
    let mut f = filter();
    assert!(!f.seek_pending(0));
    f.issued(0, PendingOp::Seek { target_ms: 5 });
    assert!(f.seek_pending(500));
    assert!(!f.seek_pending(1001));
}
