//! sppy server library: room state machine, media library, HTTP/WebSocket endpoints.

pub mod config;
pub mod error;
pub mod room;
pub mod tokens;

pub use config::{MediaRoot, ServerConfig};
pub use error::ServerError;
