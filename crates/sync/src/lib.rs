//! Pure synchronization logic for sppy: clock offset, drift control, echo filtering. No I/O.

mod clock;

pub use clock::ClockEstimator;
