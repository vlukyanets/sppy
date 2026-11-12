//! sppy server library: room state machine, media library, HTTP/WebSocket endpoints.

pub mod clock;
pub mod config;
pub mod error;
mod http;
mod hub;
pub mod paths;
mod persist;
pub mod room;
pub mod tokens;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, RwLock};

use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

pub use config::{MediaRoot, ServerConfig};
pub use error::ServerError;

use crate::clock::ServerClock;
use crate::hub::{Hub, HubInput, SharedTokens};
use crate::room::Room;

/// A running server. Dropping it does not stop the server; call [`ServerHandle::shutdown`].
pub struct ServerHandle {
    pub local_addr: SocketAddr,
    pub host_token: String,
    invite: Arc<RwLock<String>>,
    hub: mpsc::UnboundedSender<HubInput>,
    stop_http: oneshot::Sender<()>,
    http_task: JoinHandle<()>,
}

impl ServerHandle {
    pub fn invite_token(&self) -> String {
        self.invite.read().expect("invite lock").clone()
    }

    /// Tell clients we are going away, save state and stop listening.
    pub async fn shutdown(self) {
        let (done_tx, done_rx) = oneshot::channel();
        if self.hub.send(HubInput::Shutdown { done: done_tx }).is_ok() {
            let _ = done_rx.await;
        }
        let _ = self.stop_http.send(());
        let _ = self.http_task.await;
    }
}

/// Start a server: load tokens from `data_dir`, bind `config.listen`, spawn the room hub.
pub async fn start(config: ServerConfig, data_dir: PathBuf) -> Result<ServerHandle, ServerError> {
    let (tokens, created) = tokens::load_or_create(&data_dir)?;
    if created {
        tracing::info!("host token: {}", tokens.host);
        tracing::info!("invite token: {}", tokens.invite);
    }
    let listener = tokio::net::TcpListener::bind(config.listen)
        .await
        .map_err(|source| ServerError::Bind { addr: config.listen, source })?;
    let local_addr = listener.local_addr()?;

    let clock = ServerClock::new();
    let invite = Arc::new(RwLock::new(tokens.invite.clone()));
    let shared = SharedTokens { host: tokens.host.clone(), invite: invite.clone() };
    let room = match persist::load_state(&data_dir) {
        Some(saved) => Room::restore(config::room_config(&config), saved),
        None => Room::new(config::room_config(&config)),
    };
    let (hub_tx, hub_rx) = mpsc::unbounded_channel();
    tokio::spawn(Hub::new(room, clock, data_dir, shared).run(hub_rx));

    let state = http::AppState {
        hub: hub_tx.clone(),
        clock,
        ping_timeout_ms: config.ping_timeout_ms,
        next_conn_id: Arc::new(AtomicU64::new(1)),
    };
    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    let app = http::router(state);
    let http_task = tokio::spawn(async move {
        let serve = axum::serve(listener, app).with_graceful_shutdown(async move {
            let _ = stop_rx.await;
        });
        if let Err(e) = serve.await {
            tracing::error!("http server error: {e}");
        }
    });
    tracing::info!("listening on {local_addr}");
    Ok(ServerHandle {
        local_addr,
        host_token: tokens.host,
        invite,
        hub: hub_tx,
        stop_http: stop_tx,
        http_task,
    })
}
