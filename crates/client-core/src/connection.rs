//! WebSocket connection to the server: handshake, clock pings, reconnect with resume (spec §7.1).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use sppy_proto::{
    Auth, ClientMsg, CloseReason, ErrorCode, PROTOCOL_VERSION, RoomId, ServerMsg, Welcome,
    decode_server, encode,
};
use sppy_sync::ClockEstimator;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;

use crate::link::ServerAddress;

const BACKOFF_MS: [u64; 5] = [500, 1000, 2000, 5000, 10_000];
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const PING_BURST: u32 = 5;
const PING_BURST_INTERVAL: Duration = Duration::from_millis(100);
const PING_INTERVAL: Duration = Duration::from_secs(2);
/// No pong for this long means the link is dead (e.g. after laptop sleep).
const PONG_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthToken {
    Host(String),
    Invite(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConnEvent {
    Connected {
        welcome: Welcome,
    },
    Message(ServerMsg),
    /// A live connection dropped; reconnect attempts follow.
    Disconnected {
        reason: String,
    },
    /// About to retry (1-based attempt number).
    Reconnecting {
        attempt: u32,
        reason: String,
    },
    /// The server refused us for good (bad token, version mismatch, …); the task stops.
    Rejected {
        code: ErrorCode,
        message: String,
    },
}

pub struct ConnectionHandle {
    /// Requests to the server. Dropping it ends the connection with `Bye`.
    pub tx: mpsc::UnboundedSender<ClientMsg>,
    pub events: mpsc::UnboundedReceiver<ConnEvent>,
    pub clock: Arc<Mutex<ClockEstimator>>,
    pub epoch: Instant,
}

impl ConnectionHandle {
    /// Local monotonic milliseconds (the clock estimator's local time base).
    pub fn local_ms(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64
    }

    /// Current server time, once at least one ping came back.
    pub fn server_now_ms(&self) -> Option<u64> {
        self.clock.lock().expect("clock").to_server_ms(self.local_ms())
    }
}

pub fn spawn_connection(
    addr: ServerAddress,
    auth: AuthToken,
    nickname: String,
) -> ConnectionHandle {
    spawn_connection_at(addr, auth, nickname, Instant::now())
}

/// Like [`spawn_connection`], with the local time base shared with the caller.
pub fn spawn_connection_at(
    addr: ServerAddress,
    auth: AuthToken,
    nickname: String,
    epoch: Instant,
) -> ConnectionHandle {
    let (tx, out_rx) = mpsc::unbounded_channel();
    let (ev_tx, events) = mpsc::unbounded_channel();
    let clock = Arc::new(Mutex::new(ClockEstimator::new()));
    let ctx =
        Ctx { addr, auth, nickname, out_rx, ev_tx, clock: clock.clone(), epoch, session: None };
    tokio::spawn(ctx.run());
    ConnectionHandle { tx, events, clock, epoch }
}

enum Outcome {
    UserQuit,
    Final,
    RetryNow,
    Dropped(String),
    Failed(String),
}

struct Ctx {
    addr: ServerAddress,
    auth: AuthToken,
    nickname: String,
    out_rx: mpsc::UnboundedReceiver<ClientMsg>,
    ev_tx: mpsc::UnboundedSender<ConnEvent>,
    clock: Arc<Mutex<ClockEstimator>>,
    epoch: Instant,
    session: Option<String>,
}

impl Ctx {
    fn local_ms(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64
    }

    async fn run(mut self) {
        let mut attempt: u32 = 0;
        loop {
            // Requests made while offline are stale; drop them.
            loop {
                match self.out_rx.try_recv() {
                    Ok(_) => continue,
                    Err(mpsc::error::TryRecvError::Empty) => break,
                    Err(mpsc::error::TryRecvError::Disconnected) => return,
                }
            }
            let reason = match self.connect_once().await {
                Outcome::UserQuit | Outcome::Final => return,
                Outcome::RetryNow => continue,
                Outcome::Dropped(reason) => {
                    attempt = 0;
                    if self.ev_tx.send(ConnEvent::Disconnected { reason: reason.clone() }).is_err()
                    {
                        return;
                    }
                    reason
                }
                Outcome::Failed(reason) => reason,
            };
            attempt += 1;
            if self.ev_tx.send(ConnEvent::Reconnecting { attempt, reason }).is_err() {
                return;
            }
            let delay = BACKOFF_MS[(attempt as usize - 1).min(BACKOFF_MS.len() - 1)];
            let wake = tokio::time::sleep(Duration::from_millis(delay));
            tokio::pin!(wake);
            loop {
                tokio::select! {
                    _ = &mut wake => break,
                    msg = self.out_rx.recv() => if msg.is_none() { return },
                }
            }
        }
    }

    fn hello(&self) -> ClientMsg {
        let auth = match (&self.session, &self.auth) {
            (Some(s), _) => Auth::Resume { session_token: s.clone() },
            (None, AuthToken::Host(t)) => Auth::Host { token: t.clone() },
            (None, AuthToken::Invite(t)) => Auth::Invite { token: t.clone() },
        };
        ClientMsg::Hello {
            protocol: PROTOCOL_VERSION,
            room: RoomId::main(),
            auth,
            nickname: self.nickname.clone(),
            client: format!("sppy/{}", env!("CARGO_PKG_VERSION")),
        }
    }

    async fn connect_once(&mut self) -> Outcome {
        let ws = match tokio::time::timeout(
            HANDSHAKE_TIMEOUT,
            tokio_tungstenite::connect_async_with_config(self.addr.ws_url(), None, true),
        )
        .await
        {
            Ok(Ok((ws, _))) => ws,
            Ok(Err(e)) => return Outcome::Failed(e.to_string()),
            Err(_) => return Outcome::Failed("connection timed out".into()),
        };
        let (mut sink, mut stream) = ws.split();
        if sink.send(Message::text(encode(&self.hello()))).await.is_err() {
            return Outcome::Failed("cannot send hello".into());
        }
        let first = tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
            loop {
                match stream.next().await {
                    Some(Ok(Message::Text(t))) => return decode_server(t.as_str()).ok(),
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return None,
                    Some(Ok(_)) => continue,
                }
            }
        })
        .await
        .ok()
        .flatten();
        match first {
            Some(ServerMsg::Welcome(w)) => {
                self.session = Some(w.session_token.clone());
                *self.clock.lock().expect("clock") = ClockEstimator::new();
                if self.ev_tx.send(ConnEvent::Connected { welcome: w }).is_err() {
                    return Outcome::UserQuit;
                }
            }
            Some(ServerMsg::Reject { code: ErrorCode::SessionExpired, .. })
                if self.session.is_some() =>
            {
                self.session = None;
                return Outcome::RetryNow;
            }
            Some(ServerMsg::Reject { code, message }) => {
                let _ = self.ev_tx.send(ConnEvent::Rejected { code, message });
                return Outcome::Final;
            }
            _ => return Outcome::Failed("handshake failed".into()),
        }

        let mut pings_sent = 0u32;
        let mut next_ping = Instant::now();
        let mut last_pong = Instant::now();
        loop {
            tokio::select! {
                _ = tokio::time::sleep_until(next_ping) => {
                    if last_pong.elapsed() > PONG_TIMEOUT {
                        return Outcome::Dropped("server not responding".into());
                    }
                    let ping = ClientMsg::Ping { client_time_ms: self.local_ms() };
                    if sink.send(Message::text(encode(&ping))).await.is_err() {
                        return Outcome::Dropped("connection lost".into());
                    }
                    pings_sent += 1;
                    next_ping += if pings_sent < PING_BURST { PING_BURST_INTERVAL } else { PING_INTERVAL };
                }
                out = self.out_rx.recv() => match out {
                    Some(msg) => {
                        if sink.send(Message::text(encode(&msg))).await.is_err() {
                            return Outcome::Dropped("connection lost".into());
                        }
                    }
                    None => {
                        let _ = sink.send(Message::text(encode(&ClientMsg::Bye))).await;
                        let _ = sink.close().await;
                        return Outcome::UserQuit;
                    }
                },
                frame = stream.next() => match frame {
                    Some(Ok(Message::Text(t))) => match decode_server(t.as_str()) {
                        Ok(ServerMsg::Pong { client_time_ms, server_time_ms }) => {
                            last_pong = Instant::now();
                            let now = self.local_ms();
                            self.clock.lock().expect("clock").add_sample(client_time_ms, server_time_ms, now);
                        }
                        Ok(msg @ ServerMsg::Closing { reason }) => {
                            let _ = self.ev_tx.send(ConnEvent::Message(msg));
                            return if reason == CloseReason::Kicked {
                                Outcome::Final
                            } else {
                                Outcome::Dropped(format!("server closed the connection: {reason:?}"))
                            };
                        }
                        Ok(msg) => {
                            if self.ev_tx.send(ConnEvent::Message(msg)).is_err() {
                                return Outcome::UserQuit;
                            }
                        }
                        Err(e) => tracing::warn!("ignoring undecodable server message: {e}"),
                    },
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => {
                        return Outcome::Dropped("connection lost".into());
                    }
                    Some(Ok(_)) => {}
                },
            }
        }
    }
}
