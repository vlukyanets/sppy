//! Echo suppression: was a player event caused by our own command or by the person (spec §6.5)?

/// A command we sent to the player and expect to see reflected.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PendingOp {
    Pause(bool),
    /// Our seek lands within [`SEEK_MATCH_MS`] of `target_ms`.
    Seek {
        target_ms: u64,
    },
    Speed(f64),
    /// Loading a file: everything is ignored until `load_finished`.
    Load,
}

/// Player events relevant for echo detection.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RawEvent {
    Pause(bool),
    Seeked { position_ms: u64 },
    Speed(f64),
}

/// Something the person in front of the player did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserAction {
    Pause { position_ms: u64 },
    Play { position_ms: u64 },
    Seek { position_ms: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Classified {
    /// Reflection of our own command.
    Ours,
    /// The person did this: send it to the server as a request.
    User(UserAction),
    /// A speed change we did not make: put our speed back.
    RevertSpeed,
    /// Noise while loading.
    Ignored,
}

const SPEED_EPS: f64 = 0.001;
/// A `Seeked` event this close to a pending seek target is our own.
pub const SEEK_MATCH_MS: u64 = 1000;
/// Seeks can take seconds on network streams; their echo is awaited longer than the default.
pub const SEEK_TIMEOUT_MS: u64 = 10_000;

#[derive(Debug, Clone)]
pub struct EchoFilter {
    timeout_ms: u64,
    pending: Vec<(u64, PendingOp)>,
    loading: bool,
}

impl EchoFilter {
    pub fn new(timeout_ms: u64) -> Self {
        EchoFilter { timeout_ms, pending: Vec::new(), loading: false }
    }

    pub fn issued(&mut self, now_ms: u64, op: PendingOp) {
        if op == PendingOp::Load {
            self.loading = true;
            self.pending.clear();
        } else {
            self.pending.push((now_ms, op));
        }
    }

    pub fn load_finished(&mut self) {
        self.loading = false;
    }

    fn op_timeout(op: &PendingOp, default_ms: u64) -> u64 {
        match op {
            PendingOp::Seek { .. } => SEEK_TIMEOUT_MS.max(default_ms),
            _ => default_ms,
        }
    }

    /// Whether one of our own seeks is still in flight.
    pub fn seek_pending(&self, now_ms: u64) -> bool {
        self.pending.iter().any(|(t, op)| {
            matches!(op, PendingOp::Seek { .. })
                && now_ms.saturating_sub(*t) <= SEEK_TIMEOUT_MS.max(self.timeout_ms)
        })
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    pub fn observe(&mut self, now_ms: u64, ev: RawEvent, position_ms: u64) -> Classified {
        if self.loading {
            return Classified::Ignored;
        }
        let timeout = self.timeout_ms;
        self.pending.retain(|(t, op)| now_ms.saturating_sub(*t) <= Self::op_timeout(op, timeout));
        let matches = |op: &PendingOp| match (op, ev) {
            (PendingOp::Pause(a), RawEvent::Pause(b)) => *a == b,
            (PendingOp::Seek { target_ms }, RawEvent::Seeked { position_ms }) => {
                target_ms.abs_diff(position_ms) <= SEEK_MATCH_MS
            }
            (PendingOp::Speed(a), RawEvent::Speed(b)) => (a - b).abs() < SPEED_EPS,
            _ => false,
        };
        if let Some(i) = self.pending.iter().position(|(_, op)| matches(op)) {
            let (issued_at, op) = self.pending.remove(i);
            if matches!(op, PendingOp::Seek { .. }) {
                // Earlier seeks were superseded by this one (mpv reports only the last).
                self.pending
                    .retain(|(t, op)| !(matches!(op, PendingOp::Seek { .. }) && *t <= issued_at));
            }
            return Classified::Ours;
        }
        match ev {
            RawEvent::Pause(true) => Classified::User(UserAction::Pause { position_ms }),
            RawEvent::Pause(false) => Classified::User(UserAction::Play { position_ms }),
            RawEvent::Seeked { position_ms } => Classified::User(UserAction::Seek { position_ms }),
            RawEvent::Speed(_) => Classified::RevertSpeed,
        }
    }
}
