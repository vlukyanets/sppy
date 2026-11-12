//! Continuous drift correction: nothing / speed / seek (spec §6.3).

use crate::SyncParams;

/// Where the room says the player should be.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Target {
    pub position_ms: u64,
    pub playing: bool,
    pub room_speed: f64,
}

/// What the player reports.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Observed {
    pub position_ms: u64,
    pub paused: bool,
    pub speed: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Correction {
    SetSpeed(f64),
    Seek { position_ms: u64 },
    Pause,
    Play,
}

const SPEED_EPS: f64 = 0.001;

#[derive(Debug, Clone)]
pub struct DriftController {
    p: SyncParams,
    settle_until: u64,
    correcting: bool,
    seek_issued: bool,
    seek_lead_ms: u64,
    commanded_speed: Option<f64>,
    last_pause_cmd: Option<(bool, u64)>,
}

impl DriftController {
    pub fn new(p: SyncParams) -> Self {
        DriftController {
            p,
            settle_until: 0,
            correcting: false,
            seek_issued: false,
            seek_lead_ms: 0,
            commanded_speed: None,
            last_pause_cmd: None,
        }
    }

    /// Suppress corrections for `settle_ms` (after a load or a seek we did not issue).
    pub fn reset_settle(&mut self, now_ms: u64) {
        self.settle_until = now_ms + self.p.settle_ms;
        self.correcting = false;
    }

    pub fn seek_lead_ms(&self) -> u64 {
        self.seek_lead_ms
    }

    /// The speed the player should currently run at (used to revert foreign speed changes).
    pub fn desired_speed(&self, room_speed: f64) -> f64 {
        self.commanded_speed.unwrap_or(room_speed)
    }

    pub fn update(&mut self, now_ms: u64, target: &Target, observed: &Observed) -> Vec<Correction> {
        let mut out = Vec::new();
        if now_ms < self.settle_until {
            return out;
        }
        let drift = observed.position_ms as i64 - target.position_ms as i64;
        let abs = drift.unsigned_abs();
        let current_speed = self.commanded_speed.unwrap_or(observed.speed);

        if !target.playing {
            self.correcting = false;
            if !observed.paused && self.may_send_pause(true, now_ms) {
                out.push(Correction::Pause);
            }
            if (current_speed - target.room_speed).abs() > SPEED_EPS {
                self.set_speed(target.room_speed, &mut out);
            }
            if abs > self.p.dead_band_ms {
                out.push(Correction::Seek { position_ms: target.position_ms });
                self.settle_until = now_ms + self.p.settle_ms;
            }
            return out;
        }

        if observed.paused {
            if self.may_send_pause(false, now_ms) {
                out.push(Correction::Play);
            }
            if abs > self.p.seek_threshold_ms {
                out.push(Correction::Seek { position_ms: target.position_ms + self.seek_lead_ms });
                self.seek_issued = true;
            }
            self.settle_until = now_ms + self.p.settle_ms;
            self.correcting = false;
            return out;
        }

        if abs > self.p.seek_threshold_ms {
            out.push(Correction::Seek { position_ms: target.position_ms + self.seek_lead_ms });
            self.seek_issued = true;
            self.correcting = false;
            self.settle_until = now_ms + self.p.settle_ms;
            if (current_speed - target.room_speed).abs() > SPEED_EPS {
                self.set_speed(target.room_speed, &mut out);
            }
            return out;
        }

        if self.seek_issued {
            self.seek_issued = false;
            if drift < -(self.p.dead_band_ms as i64) {
                self.seek_lead_ms = (self.seek_lead_ms + abs).min(self.p.max_seek_lead_ms);
            }
        }

        let threshold = if self.correcting { self.p.speed_exit_ms } else { self.p.dead_band_ms };
        if abs < threshold {
            self.correcting = false;
            if (current_speed - target.room_speed).abs() > SPEED_EPS {
                self.set_speed(target.room_speed, &mut out);
            }
            return out;
        }

        self.correcting = true;
        let max = self.p.max_speed_correction;
        let factor = 1.0 - (drift as f64 / self.p.horizon_ms as f64).clamp(-max, max);
        let speed = target.room_speed * factor;
        if (speed - current_speed).abs() > SPEED_EPS {
            self.set_speed(speed, &mut out);
        }
        out
    }

    fn set_speed(&mut self, speed: f64, out: &mut Vec<Correction>) {
        self.commanded_speed = Some(speed);
        out.push(Correction::SetSpeed(speed));
    }

    /// Avoid re-sending the same pause/play while the player has not applied it yet.
    fn may_send_pause(&mut self, paused: bool, now_ms: u64) -> bool {
        if let Some((p, t)) = self.last_pause_cmd
            && p == paused
            && now_ms < t + self.p.settle_ms
        {
            return false;
        }
        self.last_pause_cmd = Some((paused, now_ms));
        true
    }
}
