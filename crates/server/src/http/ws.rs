//! One WebSocket connection: handshake, then pump messages between the socket and the hub.

use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use sppy_proto::{
    ClientMsg, CloseReason, ErrorCode, PROTOCOL_VERSION, ServerMsg, decode_client, encode,
};
use tokio::sync::{OwnedSemaphorePermit, mpsc, oneshot};
use tokio::time::Instant;

use super::AppState;
use crate::hub::HubInput;
use crate::rate::TokenBucket;

const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
/// Largest accepted client message (a big PlaylistAdd fits comfortably).
const MAX_MESSAGE_BYTES: usize = 1 << 20;
/// Connections that have not joined yet: each may hold its slot for [`HELLO_TIMEOUT`].
pub const MAX_PENDING_CONNECTIONS: usize = 32;
/// Messages a connection may send at once; after that one per [`MESSAGE_REFILL_MS`].
/// Far above what the app sends, even while someone holds a seek key.
pub const MAX_MESSAGE_BURST: u32 = 200;
const MESSAGE_REFILL_MS: u64 = 20;

pub async fn upgrade(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    let Ok(pending) = state.pending.clone().try_acquire_owned() else {
        tracing::warn!("too many connections waiting to join; refusing one");
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    // Client messages are small; refuse anything big before it is buffered.
    ws.max_message_size(MAX_MESSAGE_BYTES)
        .max_frame_size(MAX_MESSAGE_BYTES)
        .on_upgrade(move |socket| connection(socket, state, pending))
}

async fn send(sink: &mut SplitSink<WebSocket, Message>, msg: &ServerMsg) -> bool {
    sink.send(Message::Text(encode(msg).into())).await.is_ok()
}

/// A peer that does not read for this long is gone, even if its pings still arrive:
/// otherwise this task waits forever and the messages for it pile up.
async fn send_within(
    sink: &mut SplitSink<WebSocket, Message>,
    msg: &ServerMsg,
    limit: Duration,
) -> bool {
    tokio::time::timeout(limit, send(sink, msg)).await.unwrap_or(false)
}

async fn close_with(sink: &mut SplitSink<WebSocket, Message>, msg: ServerMsg) {
    let _ = send(sink, &msg).await;
    let _ = sink.send(Message::Close(None)).await;
}

async fn connection(socket: WebSocket, state: AppState, pending: OwnedSemaphorePermit) {
    let (mut sink, mut stream) = socket.split();

    let hello = match tokio::time::timeout(HELLO_TIMEOUT, stream.next()).await {
        Ok(Some(Ok(Message::Text(t)))) => decode_client(t.as_str()).ok(),
        _ => None,
    };
    let Some(ClientMsg::Hello { protocol, auth, nickname, .. }) = hello else {
        close_with(&mut sink, ServerMsg::Closing { reason: CloseReason::ProtocolError }).await;
        return;
    };
    if protocol != PROTOCOL_VERSION {
        let reject = ServerMsg::Reject {
            code: ErrorCode::VersionMismatch { server_protocol: PROTOCOL_VERSION },
            message: format!("server protocol v{PROTOCOL_VERSION}, client v{protocol}"),
        };
        close_with(&mut sink, reject).await;
        return;
    }

    let conn_id = state.next_conn_id.fetch_add(1, Ordering::Relaxed);
    let (out_tx, mut out_rx) = mpsc::unbounded_channel();
    let (reply_tx, reply_rx) = oneshot::channel();
    let join = HubInput::Join { auth, nickname, conn_id, out: out_tx, reply: reply_tx };
    if state.hub.send(join).is_err() {
        close_with(&mut sink, ServerMsg::Closing { reason: CloseReason::ServerShutdown }).await;
        return;
    }
    let pid = match reply_rx.await {
        Ok(Ok(pid)) => {
            drop(pending);
            pid
        }
        Ok(Err(reject)) => {
            close_with(&mut sink, reject).await;
            return;
        }
        Err(_) => {
            close_with(&mut sink, ServerMsg::Closing { reason: CloseReason::ServerShutdown }).await;
            return;
        }
    };

    let ping_timeout = Duration::from_millis(state.ping_timeout_ms);
    let mut deadline = Instant::now() + ping_timeout;
    let started = Instant::now();
    let mut rate = TokenBucket::new(0, MAX_MESSAGE_BURST, MESSAGE_REFILL_MS);
    let mut said_bye = false;
    loop {
        tokio::select! {
            out = out_rx.recv() => match out {
                Some(msg) => {
                    let closing = matches!(msg, ServerMsg::Closing { .. });
                    if !send_within(&mut sink, &msg, ping_timeout).await {
                        tracing::info!(participant = pid.0, "not reading; disconnecting");
                        break;
                    }
                    if closing {
                        let _ = sink.send(Message::Close(None)).await;
                        break;
                    }
                }
                None => break,
            },
            frame = stream.next() => {
                let elapsed = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
                if frame.is_some() && !rate.try_take(elapsed) {
                    tracing::warn!(participant = pid.0, "too many messages; disconnecting");
                    close_with(&mut sink, ServerMsg::Closing { reason: CloseReason::ProtocolError }).await;
                    break;
                }
                match frame {
                Some(Ok(Message::Text(t))) => match decode_client(t.as_str()) {
                    Ok(ClientMsg::Ping { client_time_ms }) => {
                        deadline = Instant::now() + ping_timeout;
                        let pong = ServerMsg::Pong { client_time_ms, server_time_ms: state.clock.now_ms() };
                        if !send_within(&mut sink, &pong, ping_timeout).await {
                            break;
                        }
                    }
                    Ok(ClientMsg::Bye) => {
                        said_bye = true;
                        let _ = state.hub.send(HubInput::Bye { pid });
                        break;
                    }
                    Ok(ClientMsg::Hello { .. }) | Err(_) => {
                        close_with(&mut sink, ServerMsg::Closing { reason: CloseReason::ProtocolError }).await;
                        break;
                    }
                    Ok(msg) => {
                        let _ = state.hub.send(HubInput::Msg { pid, msg });
                    }
                },
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => {}
                }
            },
            _ = tokio::time::sleep_until(deadline) => {
                tracing::info!(participant = pid.0, "ping timeout");
                break;
            }
        }
    }
    if !said_bye {
        let _ = state.hub.send(HubInput::Disconnected { pid, conn_id });
    }
}
