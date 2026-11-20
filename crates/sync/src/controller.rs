//! Continuous drift correction: nothing / speed / seek (spec §6.3).

use crate::{CatchUp, SyncParams};

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
/// Consecutive compensating re-seeks allowed after one seek.
const MAX_RESEEKS: u32 = 2;
/// How long a latecomer waits for the room to come back before catching up after all.
const LATECOMER_WAIT_MS: u64 = 3000;

#[derive(Debug, Clone)]
pub struct DriftController {
    p: SyncParams,
    catch_up: CatchUp,
    /// The room goes back to a participant this far behind (it waits for latecomers).
    room_comes_back_ms: Option<u64>,
    /// Since when this player has been waiting for the room to come back.
    waiting_since: Option<u64>,
    settle_until: u64,
    correcting: bool,
    seek_issued: bool,
    /// Drift when our last seek landed (`seek_landed`): that seek's own error.
    landed_drift: Option<i64>,
    seek_lead_ms: u64,
    reseeks: u32,
    commanded_speed: Option<f64>,
    last_pause_cmd: Option<(bool, u64)>,
}

impl DriftController {
    pub fn new(p: SyncParams) -> Self {
        DriftController {
            p,
            catch_up: CatchUp::Speed,
            room_comes_back_ms: None,
            waiting_since: None,
            settle_until: 0,
            correcting: false,
            seek_issued: false,
            landed_drift: None,
            seek_lead_ms: 0,
            reseeks: 0,
            commanded_speed: None,
            last_pause_cmd: None,
        }
    }

    /// Our seek landed (the player reports where playback restarted). The drift at this
    /// moment is the seek's own error; whatever happens later (a stall during the
    /// settle time) is ordinary drift and must not teach the seek lead.
    pub fn seek_landed(&mut self, target: &Target, position_ms: u64) {
        if self.seek_issued && self.landed_drift.is_none() {
            self.landed_drift = Some(position_ms as i64 - target.position_ms as i64);
        }
    }

    /// Suppress corrections for `settle_ms` (after a load or a seek we did not issue).
    pub fn reset_settle(&mut self, now_ms: u64) {
        self.settle_until = now_ms.saturating_add(self.p.settle_ms);
        self.correcting = false;
    }

    pub fn set_catch_up(&mut self, catch_up: CatchUp) {
        self.catch_up = catch_up;
    }

    /// Set when the room waits for latecomers: it goes back to whoever is more than this
    /// behind, so such a player does not catch up as well (that would overshoot).
    pub fn set_room_comes_back(&mut self, after_ms: Option<u64>) {
        self.room_comes_back_ms = after_ms;
    }

    pub fn catch_up(&self) -> CatchUp {
        self.catch_up
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
        if self.catch_up == CatchUp::Off {
            return out;
        }
        let drift = observed.position_ms as i64 - target.position_ms as i64;
        let abs = drift.unsigned_abs();
        let current_speed = self.commanded_speed.unwrap_or(observed.speed);

        // Play/pause state is corrected immediately, even while settling.
        if target.playing && observed.paused {
            if self.may_send_pause(false, now_ms) {
                out.push(Correction::Play);
                // Starting playback: a seek now is invisible, so fix any drift right away.
                if abs > self.p.dead_band_ms {
                    out.push(Correction::Seek {
                        position_ms: target.position_ms.saturating_add(self.seek_lead_ms),
                    });
                    self.seek_issued = true;
                    self.landed_drift = None;
                    self.reseeks = 0;
                }
                self.settle_until = now_ms.saturating_add(self.p.settle_ms);
                self.correcting = false;
            }
            return out;
        }
        if !target.playing && !observed.paused && self.may_send_pause(true, now_ms) {
            out.push(Correction::Pause);
        }
        if now_ms < self.settle_until {
            return out;
        }

        if !target.playing {
            self.correcting = false;
            if (current_speed - target.room_speed).abs() > SPEED_EPS {
                self.set_speed(target.room_speed, &mut out);
            }
            if abs > self.p.dead_band_ms {
                out.push(Correction::Seek { position_ms: target.position_ms });
                self.settle_until = now_ms.saturating_add(self.p.settle_ms);
            }
            return out;
        }

        if let Some(after) = self.room_comes_back_ms
            && drift < -i64::try_from(after).unwrap_or(i64::MAX)
        {
            let since = *self.waiting_since.get_or_insert(now_ms);
            if now_ms.saturating_sub(since) < LATECOMER_WAIT_MS {
                self.correcting = false;
                if (current_speed - target.room_speed).abs() > SPEED_EPS {
                    self.set_speed(target.room_speed, &mut out);
                }
                return out;
            }
        } else {
            self.waiting_since = None;
        }

        if abs > self.p.seek_threshold_ms {
            out.push(Correction::Seek {
                position_ms: target.position_ms.saturating_add(self.seek_lead_ms),
            });
            self.seek_issued = true;
            self.landed_drift = None;
            self.reseeks = 0;
            self.correcting = false;
            self.settle_until = now_ms.saturating_add(self.p.settle_ms);
            if (current_speed - target.room_speed).abs() > SPEED_EPS {
                self.set_speed(target.room_speed, &mut out);
            }
            return out;
        }

        if self.seek_issued {
            self.seek_issued = false;
            // Without a landing report (a player that does not tell), the drift now.
            let error = self.landed_drift.take().unwrap_or(drift);
            let error_abs = error.unsigned_abs();
            if error_abs > self.p.dead_band_ms {
                if error < 0 {
                    self.seek_lead_ms =
                        self.seek_lead_ms.saturating_add(error_abs).min(self.p.max_seek_lead_ms);
                } else {
                    self.seek_lead_ms = self.seek_lead_ms.saturating_sub(error_abs);
                }
            }
            if error_abs > self.p.reseek_threshold_ms && self.reseeks < MAX_RESEEKS {
                self.reseeks += 1;
                out.push(Correction::Seek {
                    position_ms: target.position_ms.saturating_add(self.seek_lead_ms),
                });
                self.seek_issued = true;
                self.landed_drift = None;
                self.correcting = false;
                self.settle_until = now_ms.saturating_add(self.p.settle_ms);
                return out;
            }
            self.reseeks = 0;
            // The residual of our own seek is a known error, even inside the dead band:
            // close it down to the exit threshold instead of living with it.
            if abs >= self.p.speed_exit_ms {
                self.correcting = true;
            }
        }

        if self.catch_up == CatchUp::Jumps {
            self.correcting = false;
            if (current_speed - target.room_speed).abs() > SPEED_EPS {
                self.set_speed(target.room_speed, &mut out);
            }
            if abs > self.p.jump_threshold_ms {
                out.push(Correction::Seek {
                    position_ms: target.position_ms.saturating_add(self.seek_lead_ms),
                });
                self.seek_issued = true;
                self.landed_drift = None;
                self.reseeks = 0;
                self.settle_until = now_ms.saturating_add(self.p.settle_ms);
            }
            return out;
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
        let min = self.p.min_speed_correction.min(max);
        // Proportional, but never slower than `min` so small drifts close in seconds.
        let p = (drift as f64 / self.p.horizon_ms as f64).clamp(-max, max);
        let p = if p.abs() < min { min.copysign(p) } else { p };
        let factor = 1.0 - p;
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
            && now_ms < t.saturating_add(self.p.settle_ms)
        {
            return false;
        }
        self.last_pause_cmd = Some((paused, now_ms));
        true
    }
}
