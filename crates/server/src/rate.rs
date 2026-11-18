//! A token bucket: allows bursts up to `capacity`, then one event per `refill_ms`.
//! Time is passed in, so the room can use it and stay free of clocks.

#[derive(Debug, Clone)]
pub struct TokenBucket {
    capacity: u32,
    refill_ms: u64,
    tokens: u32,
    /// When the last token was added (or the bucket was created).
    last_ms: u64,
}

impl TokenBucket {
    pub fn new(now_ms: u64, capacity: u32, refill_ms: u64) -> Self {
        TokenBucket { capacity, refill_ms: refill_ms.max(1), tokens: capacity, last_ms: now_ms }
    }

    /// Take a token if there is one.
    pub fn try_take(&mut self, now_ms: u64) -> bool {
        let earned = now_ms.saturating_sub(self.last_ms) / self.refill_ms;
        if earned > 0 {
            self.tokens = u64::from(self.tokens)
                .saturating_add(earned)
                .min(u64::from(self.capacity))
                .try_into()
                .expect("not above capacity");
            self.last_ms = if self.tokens == self.capacity {
                now_ms
            } else {
                self.last_ms + earned * self.refill_ms
            };
        }
        if self.tokens == 0 {
            return false;
        }
        self.tokens -= 1;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn burst_then_steady_rate() {
        let mut b = TokenBucket::new(0, 3, 1000);
        assert!((0..3).all(|_| b.try_take(0)));
        assert!(!b.try_take(0));
        assert!(!b.try_take(999));
        assert!(b.try_take(1000));
        assert!(!b.try_take(1500));
        // Partial time is not lost between takes.
        assert!(b.try_take(2000));
    }

    #[test]
    fn idle_time_refills_up_to_capacity_only() {
        let mut b = TokenBucket::new(0, 2, 100);
        assert!(b.try_take(0) && b.try_take(0));
        let later = 1_000_000;
        assert!(b.try_take(later) && b.try_take(later));
        assert!(!b.try_take(later));
        assert!(b.try_take(later + 100));
    }

    #[test]
    fn time_going_backwards_does_not_panic_or_refill() {
        let mut b = TokenBucket::new(5_000, 1, 100);
        assert!(b.try_take(5_000));
        assert!(!b.try_take(0));
    }
}
