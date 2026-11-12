//! Monotonic server clock: milliseconds since server start.

use tokio::time::Instant;

#[derive(Debug, Clone, Copy)]
pub struct ServerClock {
    start: Instant,
}

impl ServerClock {
    pub fn new() -> Self {
        ServerClock { start: Instant::now() }
    }

    pub fn now_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }
}

impl Default for ServerClock {
    fn default() -> Self {
        Self::new()
    }
}
