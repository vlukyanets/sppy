//! NTP-like estimate of the server clock offset (spec §6.2).

use std::collections::VecDeque;

const WINDOW: usize = 16;

#[derive(Debug, Clone, Copy)]
struct Sample {
    rtt_ms: u64,
    offset_ms: i64,
}

/// Keeps the last 16 ping samples and uses the one with the smallest round trip.
#[derive(Debug, Clone, Default)]
pub struct ClockEstimator {
    samples: VecDeque<Sample>,
}

impl ClockEstimator {
    pub fn new() -> Self {
        Self::default()
    }

    /// One ping: local send time, server time from the pong, local receive time.
    pub fn add_sample(&mut self, t_send_ms: u64, server_ms: u64, t_recv_ms: u64) {
        if t_recv_ms < t_send_ms {
            return;
        }
        let rtt_ms = t_recv_ms - t_send_ms;
        let midpoint = (t_send_ms + t_recv_ms) as i64 / 2;
        let offset_ms = server_ms as i64 - midpoint;
        self.samples.push_back(Sample { rtt_ms, offset_ms });
        while self.samples.len() > WINDOW {
            self.samples.pop_front();
        }
    }

    fn best(&self) -> Option<&Sample> {
        self.samples.iter().min_by_key(|s| s.rtt_ms)
    }

    /// `server_time − local_time`.
    pub fn offset_ms(&self) -> Option<i64> {
        self.best().map(|s| s.offset_ms)
    }

    pub fn best_rtt_ms(&self) -> Option<u64> {
        self.best().map(|s| s.rtt_ms)
    }

    pub fn to_server_ms(&self, local_ms: u64) -> Option<u64> {
        self.offset_ms().map(|o| (local_ms as i64 + o).max(0) as u64)
    }
}
