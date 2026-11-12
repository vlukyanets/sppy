//! Protocol message and state types for sppy. No I/O.

mod ids;
mod library;
mod messages;
mod notice;
mod state;

pub use ids::*;
pub use library::*;
pub use messages::*;
pub use notice::*;
pub use state::*;

/// Wire protocol version. A mismatch is rejected during the handshake.
pub const PROTOCOL_VERSION: u32 = 1;

/// Default TCP port of an sppy server.
pub const DEFAULT_PORT: u16 = 8740;
