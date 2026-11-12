//! Pure synchronization logic for sppy: clock offset, drift control, echo filtering. No I/O.

mod clock;
mod controller;
mod params;

pub use clock::ClockEstimator;
pub use controller::{Correction, DriftController, Observed, Target};
pub use params::SyncParams;
