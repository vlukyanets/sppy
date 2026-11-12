use sppy_sync::{Correction, DriftController, Observed, SyncParams, Target};

fn target(pos: u64) -> Target {
    Target { position_ms: pos, playing: true, room_speed: 1.0 }
}

fn obs(pos: u64, speed: f64) -> Observed {
    Observed { position_ms: pos, paused: false, speed }
}

fn ctl() -> DriftController {
    DriftController::new(SyncParams::default())
}

fn speed_of(c: &[Correction]) -> Option<f64> {
    c.iter().find_map(|c| match c {
        Correction::SetSpeed(s) => Some(*s),
        _ => None,
    })
}

fn approx(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

#[test]
fn defaults_match_spec() {
    let p = SyncParams::default();
    assert_eq!(
        (
            p.dead_band_ms,
            p.speed_exit_ms,
            p.horizon_ms,
            p.seek_threshold_ms,
            p.settle_ms,
            p.max_seek_lead_ms
        ),
        (80, 30, 10_000, 1500, 1000, 2000)
    );
    assert!(approx(p.max_speed_correction, 0.08));
}

#[test]
fn in_dead_band_nothing() {
    assert!(ctl().update(0, &target(10_000), &obs(10_050, 1.0)).is_empty());
    assert!(ctl().update(0, &target(10_000), &obs(9_930, 1.0)).is_empty());
}

#[test]
fn ahead_slows_down() {
    let c = ctl().update(0, &target(10_000), &obs(10_500, 1.0));
    assert!(approx(speed_of(&c).unwrap(), 0.95), "{c:?}");
}

#[test]
fn behind_speeds_up_capped() {
    let c = ctl().update(0, &target(10_000), &obs(8_800, 1.0));
    assert!(approx(speed_of(&c).unwrap(), 1.08), "{c:?}");
}

#[test]
fn speed_relative_to_room_speed() {
    let t = Target { position_ms: 10_000, playing: true, room_speed: 1.5 };
    let c = ctl().update(0, &t, &obs(10_500, 1.5));
    assert!(approx(speed_of(&c).unwrap(), 1.5 * 0.95), "{c:?}");
}

#[test]
fn hysteresis_exit_at_30() {
    let mut k = ctl();
    let c = k.update(0, &target(10_000), &obs(10_500, 1.0));
    assert!(approx(speed_of(&c).unwrap(), 0.95));
    let c = k.update(250, &target(10_000), &obs(10_050, 0.95));
    assert!(speed_of(&c).is_none_or(|s| !approx(s, 1.0)), "still correcting: {c:?}");
    let c = k.update(500, &target(10_000), &obs(10_020, 0.995));
    assert!(approx(speed_of(&c).unwrap(), 1.0), "{c:?}");
    assert!(k.update(750, &target(10_000), &obs(10_050, 1.0)).is_empty(), "back in dead band");
}

#[test]
fn no_duplicate_speed_commands() {
    let mut k = ctl();
    let c = k.update(0, &target(10_000), &obs(10_500, 1.0));
    assert!(speed_of(&c).is_some());
    // Player has not applied the speed yet; the same correction must not be re-sent.
    assert!(k.update(250, &target(10_000), &obs(10_500, 1.0)).is_empty());
}

#[test]
fn large_drift_seeks() {
    let mut k = ctl();
    let c = k.update(0, &target(10_000), &obs(8_000, 1.0));
    assert!(c.contains(&Correction::Seek { position_ms: 10_000 }), "{c:?}");
    assert!(k.update(500, &target(10_500), &obs(8_000, 1.0)).is_empty(), "settling");
    assert!(k.update(999, &target(10_999), &obs(8_000, 1.0)).is_empty(), "settling");
}

#[test]
fn seek_lead_adapts() {
    let mut k = ctl();
    k.update(0, &target(10_000), &obs(8_000, 1.0));
    k.update(1000, &target(11_000), &obs(10_700, 1.0));
    assert_eq!(k.seek_lead_ms(), 300);
    let c = k.update(5000, &target(20_000), &obs(17_000, 1.0));
    assert!(c.contains(&Correction::Seek { position_ms: 20_300 }), "{c:?}");
}

#[test]
fn seek_lead_capped_at_2000() {
    let mut k = ctl();
    let mut t = 0;
    for _ in 0..5 {
        k.update(t, &target(t + 10_000), &obs(t, 1.0));
        t += 1000;
        k.update(t, &target(t + 10_000), &obs(t + 9_000, 1.0));
        t += 1000;
    }
    assert_eq!(k.seek_lead_ms(), 2000);
}

#[test]
fn paused_room_seeks_on_drift() {
    let t = Target { position_ms: 10_000, playing: false, room_speed: 1.0 };
    let paused = |pos| Observed { position_ms: pos, paused: true, speed: 1.0 };
    assert_eq!(
        ctl().update(0, &t, &paused(10_200)),
        vec![Correction::Seek { position_ms: 10_000 }]
    );
    assert!(ctl().update(0, &t, &paused(10_050)).is_empty());
}

#[test]
fn pause_mismatch_emits_pause() {
    let t = Target { position_ms: 10_000, playing: false, room_speed: 1.0 };
    let c = ctl().update(0, &t, &obs(10_000, 1.0));
    assert!(c.contains(&Correction::Pause), "{c:?}");
}

#[test]
fn play_mismatch_emits_play_and_settles() {
    let mut k = ctl();
    let c =
        k.update(0, &target(10_000), &Observed { position_ms: 10_000, paused: true, speed: 1.0 });
    assert!(c.contains(&Correction::Play), "{c:?}");
    assert!(k.update(250, &target(10_250), &obs(10_000, 1.0)).is_empty(), "settling after unpause");
}

#[test]
fn reset_settle_blocks_corrections() {
    let mut k = ctl();
    k.reset_settle(0);
    assert!(k.update(500, &target(10_000), &obs(10_500, 1.0)).is_empty());
    assert!(!k.update(1000, &target(10_000), &obs(10_500, 1.0)).is_empty());
}
