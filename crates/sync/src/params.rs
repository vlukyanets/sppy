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
    /// Drift that maps to the full correction: factor = drift / horizon.
    pub horizon_ms: u64,
    /// Above this drift, seek instead of adjusting speed.
    pub seek_threshold_ms: u64,
    /// No corrections for this long after a seek, load or unpause.
    pub settle_ms: u64,
    pub max_seek_lead_ms: u64,
    /// After our own seek, a residual above this triggers another (compensated) seek.
    pub reseek_threshold_ms: u64,
}

impl Default for SyncParams {
    fn default() -> Self {
        SyncParams {
            dead_band_ms: 80,
            speed_exit_ms: 30,
            max_speed_correction: 0.08,
            horizon_ms: 10_000,
            seek_threshold_ms: 1500,
            settle_ms: 1000,
            max_seek_lead_ms: 2000,
            reseek_threshold_ms: 300,
        }
    }
}
