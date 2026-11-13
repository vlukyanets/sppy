//! One WebSocket connection: handshake, then pump messages between the socket and the hub.

use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use sppy_proto::{
    ClientMsg, CloseReason, ErrorCode, PROTOCOL_VERSION, ServerMsg, decode_client, encode,
};
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;

use super::AppState;
use crate::hub::HubInput;

const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
/// Largest accepted client message (a big PlaylistAdd fits comfortably).
const MAX_MESSAGE_BYTES: usize = 1 << 20;

pub async fn upgrade(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    // Client messages are small; refuse anything big before it is buffered.
    ws.max_message_size(MAX_MESSAGE_BYTES)
        .max_frame_size(MAX_MESSAGE_BYTES)
        .on_upgrade(move |socket| connection(socket, state))
}

async fn send(sink: &mut SplitSink<WebSocket, Message>, msg: &ServerMsg) -> bool {
    sink.send(Message::Text(encode(msg).into())).await.is_ok()
}

async fn close_with(sink: &mut SplitSink<WebSocket, Message>, msg: ServerMsg) {
    let _ = send(sink, &msg).await;
    let _ = sink.send(Message::Close(None)).await;
}

async fn connection(socket: WebSocket, state: AppState) {
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
        Ok(Ok(pid)) => pid,
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
    let mut said_bye = false;
    loop {
        tokio::select! {
            out = out_rx.recv() => match out {
                Some(msg) => {
                    let closing = matches!(msg, ServerMsg::Closing { .. });
                    if !send(&mut sink, &msg).await {
                        break;
                    }
                    if closing {
                        let _ = sink.send(Message::Close(None)).await;
                        break;
                    }
                }
                None => break,
            },
            frame = stream.next() => match frame {
                Some(Ok(Message::Text(t))) => match decode_client(t.as_str()) {
                    Ok(ClientMsg::Ping { client_time_ms }) => {
                        deadline = Instant::now() + ping_timeout;
                        let pong = ServerMsg::Pong { client_time_ms, server_time_ms: state.clock.now_ms() };
                        if !send(&mut sink, &pong).await {
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
