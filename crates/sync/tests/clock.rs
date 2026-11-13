use sppy_sync::ClockEstimator;

#[test]
fn no_samples_none() {
    let c = ClockEstimator::new();
    assert_eq!(c.offset_ms(), None);
    assert_eq!(c.to_server_ms(1000), None);
    assert_eq!(c.best_rtt_ms(), None);
}

#[test]
fn symmetric_rtt_offset() {
    let mut c = ClockEstimator::new();
    c.add_sample(1000, 5050, 1100);
    assert_eq!(c.offset_ms(), Some(4000));
    assert_eq!(c.best_rtt_ms(), Some(100));
    assert_eq!(c.to_server_ms(2000), Some(6000));
}

#[test]
fn min_rtt_sample_wins() {
    let mut c = ClockEstimator::new();
    c.add_sample(0, 3050, 100); // rtt 100, offset 3000
    c.add_sample(1000, 4020, 1020); // rtt 20, offset 3010
    c.add_sample(2000, 5200, 2300); // rtt 300, offset 3050
    assert_eq!(c.offset_ms(), Some(3010));
    assert_eq!(c.best_rtt_ms(), Some(20));
}

#[test]
fn window_of_16() {
    let mut c = ClockEstimator::new();
    c.add_sample(0, 3005, 10); // rtt 10, offset 3000 — best, but will fall out
    for i in 1..=16u64 {
        let t = i * 1000;
        c.add_sample(t, t + 7000 + 25, t + 50); // rtt 50, offset 7000
    }
    assert_eq!(c.offset_ms(), Some(7000));
}

#[test]
fn negative_offset() {
    let mut c = ClockEstimator::new();
    c.add_sample(10_000, 5_050, 10_100); // server clock behind local
    assert_eq!(c.offset_ms(), Some(-5_000));
    assert_eq!(c.to_server_ms(10_000), Some(5_000));
    assert_eq!(c.to_server_ms(1_000), Some(0), "saturates at 0");
}

#[test]
fn recv_before_send_ignored() {
    let mut c = ClockEstimator::new();
    c.add_sample(1000, 5000, 900);
    assert_eq!(c.offset_ms(), None);
}

#[test]
fn clock_jump_discards_inconsistent_old_samples() {
    let mut c = ClockEstimator::new();
    c.add_sample(0, 1_005, 10); // rtt 10, offset 1000
    // The machine slept 5 s: the local monotonic clock stood still, the server's did not.
    c.add_sample(1_000, 7_050, 1_100); // rtt 100, offset 6000
    assert_eq!(c.offset_ms(), Some(6_000));
}

#[test]
fn consistent_samples_are_kept() {
    let mut c = ClockEstimator::new();
    c.add_sample(0, 1_005, 10); // rtt 10, offset 1000 (±5)
    c.add_sample(1_000, 2_080, 1_100); // rtt 100, offset 1030 (±50) — consistent
    assert_eq!(c.offset_ms(), Some(1_000));
    assert_eq!(c.best_rtt_ms(), Some(10));
}
