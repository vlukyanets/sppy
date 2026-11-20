use serde::{Deserialize, Serialize};

/// Drift controller thresholds (spec §6.3); `[advanced]` in the client config.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SyncParams {
    /// No correction below this drift.
    pub dead_band_ms: u64,
    /// An active speed correction ends below this drift (hysteresis).
    pub speed_exit_ms: u64,
    /// Maximum relative speed change, e.g. 0.08 = ±8 %.
    pub max_speed_correction: f64,
    /// Minimum relative speed change while correcting, so small drifts close in seconds.
    pub min_speed_correction: f64,
    /// Drift that maps to the full correction: factor = drift / horizon.
    pub horizon_ms: u64,
    /// Above this drift, seek instead of adjusting speed.
    pub seek_threshold_ms: u64,
    /// No corrections for this long after a seek, load or unpause.
    pub settle_ms: u64,
    pub max_seek_lead_ms: u64,
    /// After our own seek, a residual above this triggers another (compensated) seek.
    pub reseek_threshold_ms: u64,
    /// In the jumps mode: seek once the drift is larger than this.
    pub jump_threshold_ms: u64,
}

/// How a player that drifted away from the room is brought back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatchUp {
    /// A few percent faster or slower until it is back; a seek for large drifts.
    #[default]
    Speed,
    /// Only seeks, for players that cannot change speed or content where it is heard.
    Jumps,
    /// The player is left alone entirely.
    Off,
}

impl Default for SyncParams {
    fn default() -> Self {
        SyncParams {
            dead_band_ms: 100,
            speed_exit_ms: 30,
            max_speed_correction: 0.08,
            min_speed_correction: 0.02,
            horizon_ms: 10_000,
            seek_threshold_ms: 1500,
            settle_ms: 1000,
            max_seek_lead_ms: 2000,
            reseek_threshold_ms: 300,
            jump_threshold_ms: 500,
        }
    }
}

/// A threshold in milliseconds, for the settings and for checking `client.toml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MsField {
    DeadBand,
    SpeedExit,
    Horizon,
    SeekThreshold,
    Settle,
    MaxSeekLead,
    ReseekThreshold,
    JumpThreshold,
}

impl MsField {
    pub const ALL: [MsField; 8] = [
        MsField::DeadBand,
        MsField::SpeedExit,
        MsField::Horizon,
        MsField::SeekThreshold,
        MsField::Settle,
        MsField::MaxSeekLead,
        MsField::ReseekThreshold,
        MsField::JumpThreshold,
    ];

    /// The key in `client.toml`.
    pub fn key(self) -> &'static str {
        match self {
            MsField::DeadBand => "dead_band_ms",
            MsField::SpeedExit => "speed_exit_ms",
            MsField::Horizon => "horizon_ms",
            MsField::SeekThreshold => "seek_threshold_ms",
            MsField::Settle => "settle_ms",
            MsField::MaxSeekLead => "max_seek_lead_ms",
            MsField::ReseekThreshold => "reseek_threshold_ms",
            MsField::JumpThreshold => "jump_threshold_ms",
        }
    }

    /// The name in the settings window.
    pub fn label(self) -> &'static str {
        match self {
            MsField::DeadBand => "Dead band",
            MsField::SpeedExit => "Speed correction exit",
            MsField::Horizon => "Speed correction horizon",
            MsField::SeekThreshold => "Seek threshold",
            MsField::Settle => "Settle time",
            MsField::MaxSeekLead => "Max seek lead",
            MsField::ReseekThreshold => "Re-seek threshold",
            MsField::JumpThreshold => "Jump threshold",
        }
    }

    /// Smallest and largest allowed value. Only the thresholds whose large values
    /// silently stop sync have a largest one: no corrections for that long after every
    /// seek or pause, drifts of minutes closed only by speed or left alone, every seek
    /// overshooting.
    pub fn limits(self) -> (u64, Option<u64>) {
        match self {
            MsField::DeadBand => (10, None),
            MsField::SpeedExit => (0, None),
            MsField::Horizon => (1_000, None),
            MsField::SeekThreshold => (100, Some(30_000)),
            MsField::Settle => (0, Some(10_000)),
            MsField::MaxSeekLead => (0, Some(10_000)),
            MsField::ReseekThreshold => (50, None),
            MsField::JumpThreshold => (100, Some(30_000)),
        }
    }

    pub fn get(self, p: &SyncParams) -> u64 {
        match self {
            MsField::DeadBand => p.dead_band_ms,
            MsField::SpeedExit => p.speed_exit_ms,
            MsField::Horizon => p.horizon_ms,
            MsField::SeekThreshold => p.seek_threshold_ms,
            MsField::Settle => p.settle_ms,
            MsField::MaxSeekLead => p.max_seek_lead_ms,
            MsField::ReseekThreshold => p.reseek_threshold_ms,
            MsField::JumpThreshold => p.jump_threshold_ms,
        }
    }

    pub fn get_mut(self, p: &mut SyncParams) -> &mut u64 {
        match self {
            MsField::DeadBand => &mut p.dead_band_ms,
            MsField::SpeedExit => &mut p.speed_exit_ms,
            MsField::Horizon => &mut p.horizon_ms,
            MsField::SeekThreshold => &mut p.seek_threshold_ms,
            MsField::Settle => &mut p.settle_ms,
            MsField::MaxSeekLead => &mut p.max_seek_lead_ms,
            MsField::ReseekThreshold => &mut p.reseek_threshold_ms,
            MsField::JumpThreshold => &mut p.jump_threshold_ms,
        }
    }
}

/// Allowed `min_speed_correction`, as a fraction.
pub const MIN_SPEED_CORRECTION_RANGE: (f64, f64) = (0.0, 0.2);
/// Allowed `max_speed_correction`, as a fraction: more than half again is no longer a
/// correction nobody notices.
pub const MAX_SPEED_CORRECTION_RANGE: (f64, f64) = (0.01, 0.5);

/// Something wrong with the thresholds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// The `client.toml` keys involved.
    pub keys: Vec<&'static str>,
    /// What is wrong, as a sentence for the settings window.
    pub message: String,
}

fn problem(keys: &[&'static str], message: String) -> Problem {
    Problem { keys: keys.to_vec(), message }
}

fn percent(fraction: f64) -> String {
    format!("{}", (fraction * 1000.0).round() / 10.0)
}

impl SyncParams {
    /// What is wrong: a value out of its limits, or thresholds that contradict each other.
    pub fn problems(&self) -> Vec<Problem> {
        let mut out = Vec::new();
        for f in MsField::ALL {
            let v = f.get(self);
            match f.limits() {
                (min, _) if v < min => out
                    .push(problem(&[f.key()], format!("{} must be at least {min} ms.", f.label()))),
                (_, Some(max)) if v > max => out
                    .push(problem(&[f.key()], format!("{} must be at most {max} ms.", f.label()))),
                _ => {}
            }
        }
        for (key, label, v, (min, max)) in [
            (
                "min_speed_correction",
                "Min speed correction",
                self.min_speed_correction,
                MIN_SPEED_CORRECTION_RANGE,
            ),
            (
                "max_speed_correction",
                "Max speed correction",
                self.max_speed_correction,
                MAX_SPEED_CORRECTION_RANGE,
            ),
        ] {
            if !(min..=max).contains(&v) {
                out.push(problem(
                    &[key],
                    format!("{label} must be from {} to {} %.", percent(min), percent(max)),
                ));
            }
        }
        if self.speed_exit_ms >= self.dead_band_ms {
            out.push(problem(
                &["speed_exit_ms", "dead_band_ms"],
                "Speed correction exit must be below the dead band.".into(),
            ));
        }
        if self.seek_threshold_ms <= self.dead_band_ms {
            out.push(problem(
                &["seek_threshold_ms", "dead_band_ms"],
                "Seek threshold must be above the dead band.".into(),
            ));
        }
        // The jumps mode looks at no dead band: a lower threshold would jump all the time.
        if self.jump_threshold_ms <= self.dead_band_ms {
            out.push(problem(
                &["jump_threshold_ms", "dead_band_ms"],
                "Jump threshold must be above the dead band.".into(),
            ));
        }
        if self.min_speed_correction > self.max_speed_correction {
            out.push(problem(
                &["min_speed_correction", "max_speed_correction"],
                "Min speed correction must not be above the max.".into(),
            ));
        }
        out
    }

    /// The thresholds with every value involved in a problem set back to its default,
    /// and the problems found.
    pub fn sanitized(&self) -> (SyncParams, Vec<Problem>) {
        let found = self.problems();
        let defaults = SyncParams::default();
        let mut p = self.clone();
        let mut problems = found.clone();
        // A value set back can clash with another one the user changed; that one goes too.
        for _ in 0..4 {
            if problems.is_empty() {
                return (p, found);
            }
            for key in problems.iter().flat_map(|pr| pr.keys.iter()) {
                p.reset(key, &defaults);
            }
            problems = p.problems();
        }
        (if problems.is_empty() { p } else { defaults }, found)
    }

    fn reset(&mut self, key: &str, defaults: &SyncParams) {
        if let Some(f) = MsField::ALL.into_iter().find(|f| f.key() == key) {
            *f.get_mut(self) = f.get(defaults);
        } else if key == "min_speed_correction" {
            self.min_speed_correction = defaults.min_speed_correction;
        } else if key == "max_speed_correction" {
            self.max_speed_correction = defaults.max_speed_correction;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_have_no_problems() {
        assert_eq!(SyncParams::default().problems(), []);
        for f in MsField::ALL {
            let (min, max) = f.limits();
            let v = f.get(&SyncParams::default());
            assert!(v >= min && max.is_none_or(|m| v <= m), "{f:?}");
        }
    }

    #[test]
    fn limits_and_contradictions_are_named() {
        let p = SyncParams {
            settle_ms: 60_000,
            dead_band_ms: 5,
            min_speed_correction: f64::NAN,
            ..SyncParams::default()
        };
        let messages: Vec<String> = p.problems().into_iter().map(|p| p.message).collect();
        assert_eq!(
            messages,
            [
                "Dead band must be at least 10 ms.",
                "Settle time must be at most 10000 ms.",
                "Min speed correction must be from 0 to 20 %.",
                "Speed correction exit must be below the dead band.",
            ]
        );
        let p = SyncParams { dead_band_ms: 600, ..SyncParams::default() };
        assert_eq!(
            p.problems(),
            [problem(
                &["jump_threshold_ms", "dead_band_ms"],
                "Jump threshold must be above the dead band.".into()
            )]
        );
    }

    #[test]
    fn sanitizing_resets_only_what_is_wrong() {
        let p = SyncParams { settle_ms: 60_000, horizon_ms: 20_000, ..SyncParams::default() };
        let (clean, found) = p.sanitized();
        assert_eq!(clean, SyncParams { horizon_ms: 20_000, ..SyncParams::default() });
        assert_eq!(found.len(), 1);
    }

    #[test]
    fn sanitizing_follows_a_clash_with_a_value_set_back() {
        // The dead band is wrong; back at 100 it clashes with the seek threshold of 100.
        let p = SyncParams { dead_band_ms: 5, seek_threshold_ms: 100, ..SyncParams::default() };
        let (clean, _) = p.sanitized();
        assert_eq!(clean.problems(), []);
        assert_eq!(clean, SyncParams::default());
        let (clean, found) = SyncParams::default().sanitized();
        assert_eq!((clean, found), (SyncParams::default(), vec![]));
    }
}
