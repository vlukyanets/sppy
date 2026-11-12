//! The room actor: one task owns the `Room` and every participant's outbound channel.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use sppy_proto::{Auth, ClientMsg, CloseReason, ErrorCode, ParticipantId, ServerMsg};
use tokio::sync::{mpsc, oneshot};

use crate::clock::ServerClock;
use crate::room::{Effect, JoinRequest, JoinRole, Joined, Room};
use crate::tokens;

const TICK: Duration = Duration::from_millis(250);

pub type Outbound = mpsc::UnboundedSender<ServerMsg>;

pub enum HubInput {
    Join {
        auth: Auth,
        nickname: String,
        conn_id: u64,
        out: Outbound,
        reply: oneshot::Sender<Result<ParticipantId, ServerMsg>>,
    },
    Msg {
        pid: ParticipantId,
        msg: ClientMsg,
    },
    Disconnected {
        pid: ParticipantId,
        conn_id: u64,
    },
    Bye {
        pid: ParticipantId,
    },
    Shutdown {
        done: oneshot::Sender<()>,
    },
}

/// Tokens shared between the hub and the server handle.
#[derive(Clone)]
pub struct SharedTokens {
    pub host: String,
    pub invite: Arc<RwLock<String>>,
}

pub struct Hub {
    room: Room,
    clock: ServerClock,
    data_dir: PathBuf,
    tokens: SharedTokens,
    conns: HashMap<ParticipantId, (u64, Outbound)>,
}

impl Hub {
    pub fn new(room: Room, clock: ServerClock, data_dir: PathBuf, tokens: SharedTokens) -> Hub {
        Hub { room, clock, data_dir, tokens, conns: HashMap::new() }
    }

    pub async fn run(mut self, mut rx: mpsc::UnboundedReceiver<HubInput>) {
        let mut tick = tokio::time::interval(TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                input = rx.recv() => match input {
                    Some(HubInput::Shutdown { done }) => {
                        self.shutdown();
                        let _ = done.send(());
                        return;
                    }
                    Some(input) => self.handle(input),
                    None => return,
                },
                _ = tick.tick() => {
                    let effects = self.room.tick(self.clock.now_ms());
                    self.apply(effects);
                }
            }
        }
    }

    fn handle(&mut self, input: HubInput) {
        let now = self.clock.now_ms();
        match input {
            HubInput::Join { auth, nickname, conn_id, out, reply } => {
                let result = match auth {
                    Auth::Host { token } if tokens::token_eq(&token, &self.tokens.host) => {
                        self.room.join(now, self.join_request(nickname, JoinRole::Host))
                    }
                    Auth::Invite { token } if tokens::token_eq(&token, &self.invite()) => {
                        self.room.join(now, self.join_request(nickname, JoinRole::Guest))
                    }
                    Auth::Resume { session_token } => self.room.resume(now, &session_token),
                    Auth::Host { .. } | Auth::Invite { .. } => Err(ErrorCode::BadToken),
                };
                match result {
                    Ok(Joined { participant_id, effects }) => {
                        // A resumed participant may still have a stale connection: replace it.
                        self.conns.insert(participant_id, (conn_id, out));
                        let _ = reply.send(Ok(participant_id));
                        self.apply(effects);
                        if self.room.is_host(participant_id) {
                            self.send(
                                participant_id,
                                ServerMsg::InviteToken { token: self.invite() },
                            );
                        }
                    }
                    Err(code) => {
                        let message = reject_message(&code);
                        let _ = reply.send(Err(ServerMsg::Reject { code, message }));
                    }
                }
            }
            HubInput::Msg { pid, msg } => {
                if !self.conns.contains_key(&pid) {
                    return;
                }
                match msg {
                    ClientMsg::RotateInvite { req_id } => self.rotate_invite(pid, req_id),
                    msg => {
                        let effects = self.room.command(now, pid, msg);
                        self.apply(effects);
                    }
                }
            }
            HubInput::Disconnected { pid, conn_id } => {
                if self.conns.get(&pid).is_some_and(|(id, _)| *id == conn_id) {
                    self.conns.remove(&pid);
                    let effects = self.room.disconnected(now, pid);
                    self.apply(effects);
                }
            }
            HubInput::Bye { pid } => {
                self.conns.remove(&pid);
                let effects = self.room.leave(now, pid);
                self.apply(effects);
            }
            HubInput::Shutdown { .. } => unreachable!("handled in run"),
        }
    }

    fn join_request(&self, nickname: String, role: JoinRole) -> JoinRequest {
        JoinRequest { nickname, role, session_token: tokens::generate_token() }
    }

    fn invite(&self) -> String {
        self.tokens.invite.read().expect("invite lock").clone()
    }

    fn rotate_invite(&mut self, pid: ParticipantId, req_id: u32) {
        if !self.room.is_host(pid) {
            let effects = self.room.denied(pid, req_id, ErrorCode::NotHost);
            self.apply(effects);
            return;
        }
        match tokens::rotate_invite(&self.data_dir) {
            Ok(t) => {
                *self.tokens.invite.write().expect("invite lock") = t.invite.clone();
                let hosts: Vec<ParticipantId> =
                    self.conns.keys().copied().filter(|p| self.room.is_host(*p)).collect();
                for h in hosts {
                    self.send(h, ServerMsg::InviteToken { token: t.invite.clone() });
                }
            }
            Err(e) => {
                tracing::error!("cannot rotate invite token: {e}");
                let effects = self.room.denied(pid, req_id, ErrorCode::InvalidRequest);
                self.apply(effects);
            }
        }
    }

    fn apply(&mut self, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::Send { to, msg } => self.send(to, msg),
                Effect::Broadcast { msg } => {
                    for (_, out) in self.conns.values() {
                        let _ = out.send(msg.clone());
                    }
                }
                Effect::Close { to, reason } => {
                    if let Some((_, out)) = self.conns.remove(&to) {
                        let _ = out.send(ServerMsg::Closing { reason });
                    }
                }
                Effect::Persist => {}
            }
        }
    }

    fn send(&self, to: ParticipantId, msg: ServerMsg) {
        if let Some((_, out)) = self.conns.get(&to) {
            let _ = out.send(msg);
        }
    }

    fn shutdown(&mut self) {
        for (_, (_, out)) in self.conns.drain() {
            let _ = out.send(ServerMsg::Closing { reason: CloseReason::ServerShutdown });
        }
    }
}

fn reject_message(code: &ErrorCode) -> String {
    match code {
        ErrorCode::BadToken => "invalid token".into(),
        ErrorCode::SessionExpired => "session expired".into(),
        ErrorCode::NicknameTaken => "nickname is already taken".into(),
        ErrorCode::RoomFull => "room is full".into(),
        ErrorCode::InvalidRequest => "invalid nickname".into(),
        other => format!("{other:?}"),
    }
}
