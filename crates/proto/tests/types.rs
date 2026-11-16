use serde_json::json;
use sppy_proto::*;

fn playback(position_ms: u64, at_server_ms: u64, paused: bool, speed: f64) -> PlaybackState {
    PlaybackState {
        item_id: Some(ItemId(1)),
        position_ms,
        at_server_ms,
        paused,
        speed,
        hold: None,
        seq: 0,
        cause: None,
    }
}

#[test]
fn expected_position_paused() {
    let p = playback(5000, 1000, true, 1.0);
    assert_eq!(p.expected_position_ms(9000), 5000);
}

#[test]
fn expected_position_playing() {
    assert_eq!(playback(5000, 1000, false, 1.0).expected_position_ms(3500), 7500);
    assert_eq!(playback(5000, 1000, false, 1.5).expected_position_ms(3500), 8750);
}

#[test]
fn expected_position_hold_freezes() {
    let mut p = playback(5000, 1000, false, 1.0);
    p.hold = Some(Hold { reason: HoldReason::ItemStart, waiting_for: vec![ParticipantId(2)] });
    assert!(!p.is_playing());
    assert_eq!(p.expected_position_ms(9000), 5000);
}

#[test]
fn expected_position_now_before_at() {
    let p = playback(5000, 1000, false, 1.0);
    assert_eq!(p.expected_position_ms(500), 5000);
}

#[test]
fn media_status_json_shape() {
    let v =
        serde_json::to_value(MediaStatus::Ready { duration_ms: Some(1000), title: None }).unwrap();
    assert_eq!(v, json!({"kind": "Ready", "duration_ms": 1000}));
}

#[test]
fn ids_are_transparent() {
    assert_eq!(serde_json::to_value(ParticipantId(7)).unwrap(), json!(7));
    assert_eq!(serde_json::to_value(FileId("ab".into())).unwrap(), json!("ab"));
}

#[test]
fn unit_enums_are_strings() {
    assert_eq!(serde_json::to_value(ControlMode::HostOnly).unwrap(), json!("HostOnly"));
}

#[test]
fn file_ids_and_subtitle_extensions_must_be_safe_as_file_names() {
    assert!(FileId("0123456789abcdef".into()).is_well_formed());
    for bad in [
        "",
        "0123456789abcde",
        "0123456789abcdef0",
        "0123456789ABCDEF",
        "../../etc/passwd",
        "/etc/passwd",
        "0123456789abcd/f",
        "..",
        "0123456789abcde\u{e9}",
    ] {
        assert!(!FileId(bad.into()).is_well_formed(), "{bad:?}");
    }
    let sub = |ext: &str| Subtitle { index: 1, label: None, ext: ext.into() };
    for good in ["srt", "ass", "idx", "sup", "webvtt"] {
        assert!(sub(good).is_well_formed(), "{good}");
    }
    for bad in ["", "../x", "s/rt", "srt\\x", ".srt", "toolongextension", "sr t"] {
        assert!(!sub(bad).is_well_formed(), "{bad:?}");
    }
}
