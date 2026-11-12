//! Closed-loop simulation: controller + a model player with latency, jitter, slow seeks and clock skew.

use sppy_sync::{Correction, DriftController, Observed, SyncParams, Target};

const STEP_MS: u64 = 10;
const TICK_MS: u64 = 250;
const CMD_LATENCY_MS: u64 = 30;
const SEEK_MS: u64 = 200;
/// Player plays this much faster than real time (0.1 %, a realistic audio clock error).
const SKEW: f64 = 1.001;

struct Lcg(u64);
impl Lcg {
    fn jitter(&mut self, amplitude: i64) -> i64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 33) % (2 * amplitude as u64 + 1)) as i64 - amplitude
    }
}

struct Model {
    pos: f64,
    speed: f64,
    paused: bool,
    seek_until: Option<u64>,
    queue: Vec<(u64, Correction)>,
}

impl Model {
    fn step(&mut self, now: u64) {
        let due: Vec<Correction> = {
            let (due, rest): (Vec<_>, Vec<_>) = self.queue.drain(..).partition(|(t, _)| *t <= now);
            self.queue = rest;
            due.into_iter().map(|(_, c)| c).collect()
        };
        for c in due {
            match c {
                Correction::SetSpeed(s) => self.speed = s,
                Correction::Pause => self.paused = true,
                Correction::Play => self.paused = false,
                Correction::Seek { position_ms } => {
                    self.pos = position_ms as f64;
                    self.seek_until = Some(now + SEEK_MS);
                }
            }
        }
        if self.seek_until.is_some_and(|t| now >= t) {
            self.seek_until = None;
        }
        if !self.paused && self.seek_until.is_none() {
            self.pos += STEP_MS as f64 * self.speed * SKEW;
        }
    }
}

/// Returns (max |drift| over the last 60 s, number of seeks).
fn run(initial_drift_ms: f64) -> (f64, usize) {
    let mut ctl = DriftController::new(SyncParams::default());
    let mut rng = Lcg(42);
    let start_target = 60_000.0;
    let mut m = Model {
        pos: start_target + initial_drift_ms,
        speed: 1.0,
        paused: false,
        seek_until: None,
        queue: vec![],
    };
    let mut seeks = 0;
    let mut worst_late = 0.0f64;
    let total_ms = 120_000;
    let mut now = 0;
    while now <= total_ms {
        let target = start_target + now as f64;
        if now % TICK_MS == 0 {
            let observed = Observed {
                position_ms: (m.pos as i64 + rng.jitter(15)).max(0) as u64,
                paused: m.paused,
                speed: m.speed,
            };
            let t = Target { position_ms: target as u64, playing: true, room_speed: 1.0 };
            for c in ctl.update(now, &t, &observed) {
                if matches!(c, Correction::Seek { .. }) {
                    seeks += 1;
                }
                m.queue.push((now + CMD_LATENCY_MS, c));
            }
        }
        m.step(now);
        if now >= total_ms - 60_000 {
            worst_late =
                worst_late.max((m.pos - (start_target + now as f64 + STEP_MS as f64)).abs());
        }
        now += STEP_MS;
    }
    (worst_late, seeks)
}

#[test]
fn converges_without_oscillation() {
    let (worst, seeks) = run(900.0);
    eprintln!("900: worst={worst:.1} seeks={seeks}");
    assert!(worst < 80.0, "worst drift in the last 60 s: {worst}");
    assert!(seeks < 3, "seeks: {seeks}");
}

#[test]
fn recovers_from_5s_behind() {
    let (worst, seeks) = run(-5_000.0);
    eprintln!("-5000: worst={worst:.1} seeks={seeks}");
    assert!(worst < 80.0, "worst drift in the last 60 s: {worst}");
    assert!(seeks < 3, "seeks: {seeks}");
}

#[test]
fn recovers_from_3s_ahead() {
    let (worst, seeks) = run(3_000.0);
    assert!(worst < 80.0, "worst drift in the last 60 s: {worst}");
    assert!(seeks < 3, "seeks: {seeks}");
}
