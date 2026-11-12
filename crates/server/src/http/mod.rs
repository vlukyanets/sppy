//! HTTP layer: one axum router on one port.

mod ws;

use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use axum::Router;
use axum::routing::get;
use tokio::sync::mpsc;

use crate::clock::ServerClock;
use crate::hub::HubInput;

#[derive(Clone)]
pub struct AppState {
    pub hub: mpsc::UnboundedSender<HubInput>,
    pub clock: ServerClock,
    pub ping_timeout_ms: u64,
    pub next_conn_id: Arc<AtomicU64>,
}

pub fn router(state: AppState) -> Router {
    Router::new().route("/ws", get(ws::upgrade)).with_state(state)
}
