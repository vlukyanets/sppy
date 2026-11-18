//! HTTP layer: one axum router on one port.

mod media;
mod ws;

pub use ws::{MAX_MESSAGE_BURST, MAX_PENDING_CONNECTIONS};

use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use axum::Router;
use axum::routing::get;
use tokio::sync::{Semaphore, mpsc};

use crate::clock::ServerClock;
use crate::hub::HubInput;
use crate::media::SharedMedia;

#[derive(Clone)]
pub struct AppState {
    pub hub: mpsc::UnboundedSender<HubInput>,
    pub clock: ServerClock,
    pub ping_timeout_ms: u64,
    pub next_conn_id: Arc<AtomicU64>,
    pub media: Arc<SharedMedia>,
    /// Slots for connections that have not joined yet.
    pub pending: Arc<Semaphore>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/ws", get(ws::upgrade))
        .route("/media/{file_id}", get(media::file))
        .route("/media/{file_id}/sub/{index}", get(media::subtitle))
        .route("/media/{file_id}/sub/{index}/companion", get(media::subtitle_companion))
        .with_state(state)
}
