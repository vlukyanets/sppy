//! The room actor: one task owns the `Room` and every participant's outbound channel.

use sppy_common::config::MediaRoot;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use sppy_common::folders;
use sppy_proto::{
    Auth, ClientMsg, CloseReason, ErrorCode, MediaFolder, MediaFolderProblem, ParticipantId,
    ServerMsg,
};
use tokio::sync::{mpsc, oneshot};

use crate::clock::ServerClock;
use crate::library::Library;
use crate::media::{SharedMedia, scan_with_fingerprints};
use crate::room::{Effect, JoinRequest, JoinRole, Joined, Room};
use crate::tokens;

const TICK: Duration = Duration::from_millis(250);
const SAVE_MIN_INTERVAL_MS: u64 = 5_000;
const SAVE_PLAYING_INTERVAL_MS: u64 = 10_000;

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
    /// A (re)scan with fingerprints finished; scans older than the last started are dropped.
    LibraryUpdated {
        generation: u64,
        library: Library,
    },
    /// The embedding app replaced the shared folders.
    SetMediaFolders(Vec<MediaRoot>),
}

/// The shared folders and how they may change.
pub struct Folders {
    pub roots: Vec<MediaRoot>,
    /// The host may change them over the protocol.
    pub editable: bool,
    /// Where changes made by the host are saved.
    pub config_file: Option<PathBuf>,
    /// Tests: wait this long before hashing.
    pub hash_delay_ms: u64,
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
    media: Arc<SharedMedia>,
    folders: Folders,
    scan_generation: u64,
    self_tx: mpsc::UnboundedSender<HubInput>,
    dirty: bool,
    last_save_ms: Option<u64>,
}

impl Hub {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        room: Room,
        clock: ServerClock,
        data_dir: PathBuf,
        tokens: SharedTokens,
        media: Arc<SharedMedia>,
        folders: Folders,
        self_tx: mpsc::UnboundedSender<HubInput>,
    ) -> Hub {
        Hub {
            room,
            clock,
            data_dir,
            tokens,
            conns: HashMap::new(),
            media,
            folders,
            scan_generation: 0,
            self_tx,
            dirty: false,
            last_save_ms: None,
        }
    }

    /// Hash fingerprints / rescan in the background; the result comes back as `LibraryUpdated`.
    pub fn spawn_scan(&mut self) {
        self.scan_generation += 1;
        let generation = self.scan_generation;
        let roots = self.folders.roots.clone();
        let data_dir = self.data_dir.clone();
        let tx = self.self_tx.clone();
        let delay = Duration::from_millis(self.folders.hash_delay_ms);
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            match tokio::task::spawn_blocking(move || scan_with_fingerprints(&roots, &data_dir))
                .await
            {
                Ok(lib) => {
                    let _ = tx.send(HubInput::LibraryUpdated { generation, library: lib });
                }
                Err(e) => tracing::error!("library scan failed: {e}"),
            }
        });
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
                    Some(input) => {
                        self.handle(input);
                        self.sync_sessions();
                    }
                    None => return,
                },
                _ = tick.tick() => {
                    let now = self.clock.now_ms();
                    let effects = self.room.tick(now);
                    self.apply(effects);
                    self.sync_sessions();
                    self.maybe_save(now);
                }
            }
        }
    }

    fn handle(&mut self, input: HubInput) {
        let now = self.clock.now_ms();
        match input {
            HubInput::Join { auth, nickname, conn_id, out, reply } => {
                let result = match auth {
                    // A link carries one token: the host token in it makes you the host.
                    Auth::Host { token } | Auth::Invite { token }
                        if tokens::token_eq(&token, &self.tokens.host) =>
                    {
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
                            self.send(participant_id, self.folders_msg());
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
                    ClientMsg::GetLibrary => {
                        let roots = self.media.library.read().expect("library lock").tree();
                        self.send(pid, ServerMsg::Library { roots });
                    }
                    ClientMsg::AddMediaFolder { req_id, path, name } => {
                        let result = self.check_change(pid).and_then(|()| {
                            let path = std::path::Path::new(&path);
                            let root =
                                folders::new_folder(&self.folders.roots, path, name.as_deref())?;
                            let mut roots = self.folders.roots.clone();
                            roots.push(root);
                            Ok(roots)
                        });
                        self.change_folders(pid, req_id, result);
                    }
                    ClientMsg::RemoveMediaFolder { req_id, name } => {
                        let result = self.check_change(pid).and_then(|()| {
                            let mut roots = self.folders.roots.clone();
                            let before = roots.len();
                            roots.retain(|r| r.name != name);
                            if roots.len() == before {
                                return Err(ErrorCode::MediaFolder {
                                    problem: MediaFolderProblem::UnknownFolder,
                                });
                            }
                            Ok(roots)
                        });
                        self.change_folders(pid, req_id, result);
                    }
                    ClientMsg::RescanLibrary { req_id } => {
                        if self.room.is_host(pid) {
                            self.spawn_scan();
                        } else {
                            let effects = self.room.denied(pid, req_id, ErrorCode::NotHost);
                            self.apply(effects);
                        }
                    }
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
            HubInput::SetMediaFolders(roots) => self.set_folders(roots),
            HubInput::LibraryUpdated { generation, library: lib } => {
                if generation != self.scan_generation {
                    return;
                }
                let infos = lib.file_infos();
                let roots = lib.tree();
                *self.media.library.write().expect("library lock") = lib;
                let effects = self.room.set_library(now, infos);
                self.apply(effects);
                self.apply(vec![Effect::Broadcast { msg: ServerMsg::Library { roots } }]);
            }
            HubInput::Shutdown { .. } => unreachable!("handled in run"),
        }
    }

    /// Hosts may change folders only when the server allows it.
    fn check_change(&self, pid: ParticipantId) -> Result<(), ErrorCode> {
        if !self.room.is_host(pid) {
            return Err(ErrorCode::NotHost);
        }
        if !self.folders.editable {
            return Err(ErrorCode::MediaFolder { problem: MediaFolderProblem::ChangesDisabled });
        }
        Ok(())
    }

    /// Save the new folder list (if there is a config file), then use it.
    fn change_folders(
        &mut self,
        pid: ParticipantId,
        req_id: sppy_proto::ReqId,
        roots: Result<Vec<MediaRoot>, ErrorCode>,
    ) {
        let saved = roots.and_then(|roots| match &self.folders.config_file {
            Some(file) => folders::save_media_to_file(file, &roots).map(|()| roots).map_err(|e| {
                tracing::error!("cannot save {}: {e}", file.display());
                ErrorCode::MediaFolder { problem: MediaFolderProblem::CannotSave }
            }),
            None => Ok(roots),
        });
        match saved {
            Ok(roots) => self.set_folders(roots),
            Err(code) => {
                let effects = self.room.denied(pid, req_id, code);
                self.apply(effects);
            }
        }
    }

    fn set_folders(&mut self, roots: Vec<MediaRoot>) {
        tracing::info!("shared folders: {:?}", roots.iter().map(|r| &r.name).collect::<Vec<_>>());
        self.folders.roots = roots;
        self.spawn_scan();
        let msg = self.folders_msg();
        let hosts: Vec<ParticipantId> =
            self.conns.keys().copied().filter(|pid| self.room.is_host(*pid)).collect();
        for pid in hosts {
            self.send(pid, msg.clone());
        }
    }

    fn folders_msg(&self) -> ServerMsg {
        let folders = self
            .folders
            .roots
            .iter()
            .map(|r| MediaFolder {
                name: r.name.clone(),
                path: r.path.to_string_lossy().into_owned(),
                available: r.path.is_dir(),
            })
            .collect();
        ServerMsg::MediaFolders { folders, editable: self.folders.editable }
    }

    /// Keep the `/media` token set equal to the room's sessions.
    fn sync_sessions(&self) {
        let current: std::collections::HashSet<String> =
            self.room.session_tokens().map(str::to_owned).collect();
        let mut shared = self.media.sessions.write().expect("sessions lock");
        if *shared != current {
            *shared = current;
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
                Effect::Persist => self.dirty = true,
            }
        }
    }

    fn send(&self, to: ParticipantId, msg: ServerMsg) {
        if let Some((_, out)) = self.conns.get(&to) {
            let _ = out.send(msg);
        }
    }

    /// Debounced save: on change at most every 5 s; every 10 s while playing (position).
    fn maybe_save(&mut self, now: u64) {
        let since = self.last_save_ms.map(|t| now.saturating_sub(t));
        let due = match since {
            None => self.dirty,
            Some(since) => {
                (self.dirty && since >= SAVE_MIN_INTERVAL_MS)
                    || (self.room.playback().is_playing() && since >= SAVE_PLAYING_INTERVAL_MS)
            }
        };
        if due {
            self.save(now);
        }
    }

    fn save(&mut self, now: u64) {
        match crate::persist::save_state(&self.data_dir, &self.room.persisted(now)) {
            Ok(()) => {
                self.dirty = false;
                self.last_save_ms = Some(now);
            }
            Err(e) => tracing::error!("cannot save room state: {e}"),
        }
    }

    fn shutdown(&mut self) {
        self.save(self.clock.now_ms());
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
