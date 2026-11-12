//! The client session loop: connection + player + sync, in one task (spec §7.1).

use std::time::Duration;

use sppy_player::{MediaSource, Player, PlayerEvent, PlayerLauncher};
use sppy_proto::{
    ClientMsg, CloseReason, Delivery, ErrorCode, Event, ItemId, MediaStatus, PlaybackState, ReqId,
    ServerMsg,
};
use sppy_sync::{
    Classified, Correction, DriftController, EchoFilter, Observed, PendingOp, RawEvent, Target,
    UserAction,
};
use tokio::sync::{broadcast, mpsc, watch};
use tokio::time::Instant;

use crate::api::{ConnState, Snapshot, UiCommand, UiEvent};
use crate::config::ClientConfig;
use crate::connection::{AuthToken, ConnEvent, ConnectionHandle, spawn_connection_at};
use crate::link::{ServerAddress, parse_address};
use crate::media::{Resolved, stream_source};
use crate::notice::{nickname, render_notice};

const TICK: Duration = Duration::from_millis(250);
const ECHO_TIMEOUT_MS: u64 = 1000;
/// After a user action, leave the player alone until the server answers (or this long).
const AWAIT_REPLY_MS: u64 = 2000;
/// How long to wait for the `Seeked` of a position jump we did not cause.
const FOREIGN_JUMP_GRACE_MS: u64 = 1500;
const CRASH_WINDOW: Duration = Duration::from_secs(60);
const OSD_MS: u64 = 3000;
const CHAT_HISTORY: usize = 50;
pub(crate) const NO_PERMISSION: &str = "No permission to control playback";

/// What the player currently has and reports.
#[derive(Debug, Clone)]
struct MediaState {
    item: Option<ItemId>,
    file_loaded: bool,
    pos_ms: u64,
    pos_at_ms: u64,
    paused: bool,
    speed: f64,
    buffering: bool,
    cache_ms: u64,
    cache_eof: bool,
    local: bool,
    /// A position jump we did not cause: probably the user seeking; hold corrections until then.
    foreign_jump_until: u64,
    duration_ms: Option<u64>,
    title: Option<String>,
    status: MediaStatus,
}

impl Default for MediaState {
    fn default() -> Self {
        MediaState {
            item: None,
            file_loaded: false,
            pos_ms: 0,
            pos_at_ms: 0,
            paused: true,
            speed: 1.0,
            buffering: false,
            cache_ms: 0,
            cache_eof: false,
            local: false,
            foreign_jump_until: 0,
            duration_ms: None,
            title: None,
            status: MediaStatus::Loading,
        }
    }
}

pub(crate) struct Session<L: PlayerLauncher> {
    config: ClientConfig,
    launcher: L,
    epoch: Instant,
    snap: Snapshot,
    state_tx: watch::Sender<Snapshot>,
    events_tx: broadcast::Sender<UiEvent>,
    conn: Option<ConnectionHandle>,
    session_token: Option<String>,
    http_base: Option<String>,
    player: Option<L::P>,
    last_exit_code: Option<Option<i32>>,
    last_crash: Option<Instant>,
    controller: DriftController,
    echo: EchoFilter,
    next_req: ReqId,
    awaiting: Option<(ReqId, u64)>,
    media: MediaState,
}

async fn next_conn_event(conn: &mut Option<ConnectionHandle>) -> Option<ConnEvent> {
    match conn {
        Some(c) => c.events.recv().await,
        None => std::future::pending().await,
    }
}

async fn next_player_event<P: Player>(player: &mut Option<P>) -> Option<PlayerEvent> {
    match player {
        Some(p) => p.next_event().await,
        None => std::future::pending().await,
    }
}

impl<L: PlayerLauncher> Session<L> {
    pub(crate) fn new(
        config: ClientConfig,
        launcher: L,
        epoch: Instant,
        state_tx: watch::Sender<Snapshot>,
        events_tx: broadcast::Sender<UiEvent>,
    ) -> Self {
        let controller = DriftController::new(config.advanced.clone());
        let snap = state_tx.borrow().clone();
        Session {
            snap,
            config,
            launcher,
            epoch,
            state_tx,
            events_tx,
            conn: None,
            session_token: None,
            http_base: None,
            player: None,
            last_exit_code: None,
            last_crash: None,
            controller,
            echo: EchoFilter::new(ECHO_TIMEOUT_MS),
            next_req: 0,
            awaiting: None,
            media: MediaState::default(),
        }
    }

    pub(crate) async fn run(mut self, mut cmd_rx: mpsc::UnboundedReceiver<UiCommand>) {
        let mut tick = tokio::time::interval(TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                cmd = cmd_rx.recv() => match cmd {
                    Some(cmd) => self.on_command(cmd).await,
                    None => return,
                },
                ev = next_conn_event(&mut self.conn) => self.on_conn_event(ev).await,
                ev = next_player_event(&mut self.player) => self.on_player_event(ev).await,
                _ = tick.tick() => self.on_tick().await,
            }
            self.publish();
        }
    }

    fn local_ms(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64
    }

    fn publish(&mut self) {
        let now = self.local_ms();
        self.snap.player_position_ms = self.media.file_loaded.then(|| self.observed_position(now));
        self.snap.server_offset_ms =
            self.conn.as_ref().and_then(|c| c.clock.lock().expect("clock").offset_ms());
        self.state_tx.send_replace(self.snap.clone());
    }

    fn emit(&self, ev: UiEvent) {
        let _ = self.events_tx.send(ev);
    }

    fn send(&self, msg: ClientMsg) {
        if let Some(c) = &self.conn {
            let _ = c.tx.send(msg);
        }
    }

    fn request(&mut self, make: impl FnOnce(ReqId) -> ClientMsg) -> ReqId {
        self.next_req += 1;
        let req = self.next_req;
        self.send(make(req));
        req
    }

    async fn osd(&mut self, text: &str) {
        if let Some(p) = &mut self.player {
            let _ = p.osd(text, OSD_MS).await;
        }
    }

    // ---- UI commands -------------------------------------------------------------------

    async fn on_command(&mut self, cmd: UiCommand) {
        match cmd {
            UiCommand::Connect { address, nickname } => match parse_address(&address) {
                Ok(addr) => match addr.token.clone() {
                    Some(token) => {
                        self.config.nickname = nickname;
                        self.start_connection(addr, AuthToken::Invite(token));
                    }
                    None => self.emit(UiEvent::Error("The invite link has no token".into())),
                },
                Err(e) => self.emit(UiEvent::Error(e.to_string())),
            },
            UiCommand::BecomeHostConnect { port, host_token } => {
                let addr =
                    ServerAddress { secure: false, host: "127.0.0.1".into(), port, token: None };
                self.start_connection(addr, AuthToken::Host(host_token));
            }
            UiCommand::Disconnect => {
                self.conn = None;
                self.snap.connection = ConnState::Idle;
                self.snap.room = None;
                self.snap.playback = None;
            }
            UiCommand::Play => {
                self.request(|req_id| ClientMsg::Play { req_id, position_ms: None });
            }
            UiCommand::Pause => {
                self.request(|req_id| ClientMsg::Pause { req_id, position_ms: None });
            }
            UiCommand::Seek(position_ms) => {
                self.request(|req_id| ClientMsg::Seek { req_id, position_ms });
            }
            UiCommand::SetSpeed(speed) => {
                self.request(|req_id| ClientMsg::SetSpeed { req_id, speed });
            }
            UiCommand::PlaylistAdd(sources) => {
                let after =
                    self.snap.room.as_ref().and_then(|r| r.playlist.last()).map(|i| i.item_id);
                self.request(|req_id| ClientMsg::PlaylistAdd { req_id, sources, after });
            }
            UiCommand::PlaylistRemove(item_id) => {
                self.request(|req_id| ClientMsg::PlaylistRemove { req_id, item_id });
            }
            UiCommand::PlaylistMove { item_id, after } => {
                self.request(|req_id| ClientMsg::PlaylistMove { req_id, item_id, after });
            }
            UiCommand::PlaylistSelect(item_id) => {
                self.request(|req_id| ClientMsg::PlaylistSelect { req_id, item_id });
            }
            UiCommand::Chat(text) => self.send(ClientMsg::Chat { text }),
            UiCommand::GetLibrary => self.send(ClientMsg::GetLibrary),
            UiCommand::SetControlMode(mode) => {
                self.request(|req_id| ClientMsg::SetControlMode { req_id, mode });
            }
            UiCommand::SetCanControl(participant_id, allowed) => {
                self.request(|req_id| ClientMsg::SetCanControl { req_id, participant_id, allowed });
            }
            UiCommand::Kick(participant_id) => {
                self.request(|req_id| ClientMsg::Kick { req_id, participant_id });
            }
            UiCommand::RotateInvite => {
                self.request(|req_id| ClientMsg::RotateInvite { req_id });
            }
            UiCommand::RescanLibrary => {
                self.request(|req_id| ClientMsg::RescanLibrary { req_id });
            }
            UiCommand::OpenPlayer => self.ensure_player().await,
            UiCommand::SetMode(mode) => {
                self.config.mode = mode;
                self.snap.local.mode = mode;
            }
            UiCommand::ChooseLocalFile { .. } | UiCommand::ClearCache => {
                tracing::warn!("not supported yet: {cmd:?}");
            }
        }
    }

    fn start_connection(&mut self, addr: ServerAddress, auth: AuthToken) {
        self.http_base = Some(addr.http_base());
        self.conn = Some(spawn_connection_at(addr, auth, self.config.nickname.clone(), self.epoch));
        self.snap.connection = ConnState::Connecting;
    }

    // ---- connection events ---------------------------------------------------------------

    async fn on_conn_event(&mut self, ev: Option<ConnEvent>) {
        let Some(ev) = ev else {
            self.conn = None;
            if !matches!(self.snap.connection, ConnState::Rejected(_)) {
                self.snap.connection = ConnState::Idle;
            }
            return;
        };
        match ev {
            ConnEvent::Connected { welcome } => {
                self.snap.connection = ConnState::Connected;
                self.snap.me = Some(welcome.participant_id);
                self.snap.is_host = welcome.is_host;
                self.snap.room = Some(welcome.room);
                self.snap.playback = Some(welcome.playback);
                self.snap.chat_history = welcome.chat_history;
                self.session_token = Some(welcome.session_token);
                self.awaiting = None;
                if self.player.is_none() {
                    self.ensure_player().await;
                } else {
                    // A fresh participant on the server starts as Loading: tell it where we are.
                    if self.media.item.is_some() && self.media.item == self.current_item_id() {
                        self.send_status();
                    }
                    self.apply_playback().await;
                }
            }
            ConnEvent::Message(msg) => self.on_server_msg(msg).await,
            ConnEvent::Disconnected { reason } => {
                tracing::info!("disconnected: {reason}");
                self.snap.connection = ConnState::Reconnecting { attempt: 0 };
                self.osd("Connection lost").await;
            }
            ConnEvent::Reconnecting { attempt, reason } => {
                tracing::debug!("reconnecting (attempt {attempt}): {reason}");
                self.snap.connection = ConnState::Reconnecting { attempt };
            }
            ConnEvent::Rejected { code, message } => {
                self.snap.connection = ConnState::Rejected(code);
                self.emit(UiEvent::Error(message));
            }
        }
    }

    async fn on_server_msg(&mut self, msg: ServerMsg) {
        match msg {
            ServerMsg::Playback(p) => {
                if let (Some((req, _)), Some(cause)) = (self.awaiting, p.cause)
                    && Some(cause.participant_id) == self.snap.me
                    && cause.req_id == req
                {
                    self.awaiting = None;
                }
                self.snap.playback = Some(p);
                self.apply_playback().await;
            }
            ServerMsg::Room(r) => {
                if let Some(me) = self.snap.me
                    && let Some(p) = r.participants.iter().find(|p| p.id == me)
                {
                    self.snap.is_host = p.is_host;
                }
                self.snap.room = Some(r);
            }
            ServerMsg::ProgressBatch { entries } => {
                self.snap.progress = entries.into_iter().map(|e| (e.participant_id, e)).collect();
            }
            ServerMsg::Event(Event::Chat(c)) => {
                let line = format!("{}: {}", c.nickname, c.text);
                self.snap.chat_history.push(c.clone());
                if self.snap.chat_history.len() > CHAT_HISTORY {
                    self.snap.chat_history.remove(0);
                }
                self.emit(UiEvent::Chat(c));
                self.osd(&line).await;
            }
            ServerMsg::Event(Event::Notice { actor, action }) => {
                let Some(room) = &self.snap.room else { return };
                let actor = actor.map(|a| nickname(room, a));
                let text = render_notice(actor.as_deref(), &action, room);
                self.emit(UiEvent::Notice(text.clone()));
                self.osd(&text).await;
            }
            ServerMsg::Denied { req_id, code } => {
                if self.awaiting.is_some_and(|(r, _)| r == req_id) {
                    self.awaiting = None;
                }
                let text = match code {
                    ErrorCode::NotPermitted => NO_PERMISSION.to_owned(),
                    ErrorCode::NotHost => "Only the host can do that".to_owned(),
                    other => format!("Request rejected: {other:?}"),
                };
                self.osd(&text).await;
                self.emit(UiEvent::Error(text));
            }
            ServerMsg::Library { roots } => self.snap.library = Some(roots),
            ServerMsg::InviteToken { token } => self.snap.invite_token = Some(token),
            ServerMsg::Closing { reason } => match reason {
                CloseReason::Kicked => {
                    self.emit(UiEvent::Error("You were removed from the room".into()));
                    self.snap.connection = ConnState::Idle;
                }
                CloseReason::ServerShutdown => {
                    self.emit(UiEvent::Notice("The server is shutting down".into()));
                }
                CloseReason::ProtocolError => {
                    self.emit(UiEvent::Error("Protocol error".into()));
                }
            },
            ServerMsg::Welcome(_) | ServerMsg::Reject { .. } | ServerMsg::Pong { .. } => {}
        }
    }

    fn current_item_id(&self) -> Option<ItemId> {
        self.snap.playback.as_ref().and_then(|p| p.item_id)
    }

    /// Load the room's current item if the player has something else.
    async fn apply_playback(&mut self) {
        let wanted = self.current_item_id();
        if wanted == self.media.item || self.player.is_none() {
            return;
        }
        let Some(id) = wanted else {
            self.media = MediaState::default();
            return;
        };
        let Some(item) = self
            .snap
            .room
            .as_ref()
            .and_then(|r| r.playlist.iter().find(|i| i.item_id == id))
            .cloned()
        else {
            return;
        };
        let (Some(base), Some(token)) = (&self.http_base, &self.session_token) else { return };
        let (media, subs, delivery) = match stream_source(base, &item, token) {
            Resolved::Stream { media, subtitles } => (media, subtitles, Delivery::Stream),
            Resolved::Local { path, subtitles } => (
                MediaSource::Path(path),
                subtitles.into_iter().map(MediaSource::Path).collect(),
                Delivery::Local,
            ),
            Resolved::Wait => return,
        };
        self.media = MediaState {
            item: Some(id),
            local: matches!(media, MediaSource::Path(_)),
            ..MediaState::default()
        };
        self.echo.issued(self.local_ms(), PendingOp::Load);
        if let Some(p) = &mut self.player
            && let Err(e) = p.load(media, subs).await
        {
            tracing::warn!("load failed: {e}");
        }
        self.send_status();
        self.send(ClientMsg::Progress { item_id: id, delivery });
    }

    fn send_status(&self) {
        if let Some(item_id) = self.media.item {
            self.send(ClientMsg::SetStatus { item_id, status: self.media.status.clone() });
        }
    }

    fn set_status(&mut self, status: MediaStatus) {
        if self.media.status != status {
            self.media.status = status;
            self.send_status();
        }
    }

    /// Loading/Stalled → Ready once there is a reserve (spec §6.4).
    fn update_status(&mut self) {
        if !self.media.file_loaded || self.media.buffering {
            return;
        }
        if !matches!(self.media.status, MediaStatus::Loading | MediaStatus::Stalled) {
            return;
        }
        let reserve_ms = self
            .snap
            .room
            .as_ref()
            .map(|r| u64::from(r.settings.resume_buffer_secs) * 1000)
            .unwrap_or(10_000);
        if self.media.local || self.media.cache_eof || self.media.cache_ms >= reserve_ms {
            let status = MediaStatus::Ready {
                duration_ms: self.media.duration_ms,
                title: self.media.title.clone(),
            };
            self.set_status(status);
        }
    }

    // ---- player ----------------------------------------------------------------------------

    async fn ensure_player(&mut self) {
        if self.player.is_some() {
            return;
        }
        match self.launcher.launch().await {
            Ok(p) => {
                self.player = Some(p);
                self.snap.player_open = true;
                self.last_exit_code = None;
                self.media = MediaState::default();
                self.echo = EchoFilter::new(ECHO_TIMEOUT_MS);
                self.controller = DriftController::new(self.config.advanced.clone());
                self.apply_playback().await;
            }
            Err(e) => {
                self.snap.player_open = false;
                self.emit(UiEvent::Error(format!("Cannot start the player: {e}")));
            }
        }
    }

    fn player_closed(&mut self) {
        self.snap.player_open = false;
        if let Some(item_id) = self.media.item {
            self.send(ClientMsg::SetStatus { item_id, status: MediaStatus::PlayerClosed });
        }
        self.media = MediaState::default();
    }

    async fn on_player_event(&mut self, ev: Option<PlayerEvent>) {
        let Some(ev) = ev else {
            self.player = None;
            let code = self.last_exit_code.take().flatten();
            if code == Some(0) {
                self.player_closed();
            } else if self.last_crash.is_some_and(|t| t.elapsed() < CRASH_WINDOW) {
                tracing::warn!("player crashed again (exit code {code:?})");
                self.player_closed();
            } else {
                tracing::warn!("player crashed (exit code {code:?}), restarting");
                self.last_crash = Some(Instant::now());
                self.ensure_player().await;
            }
            return;
        };
        let now = self.local_ms();
        match ev {
            PlayerEvent::TimePos(ms) => {
                let jump = ms.abs_diff(self.observed_position(now));
                if self.media.file_loaded
                    && jump > self.config.advanced.seek_threshold_ms
                    && !self.echo.seek_pending(now)
                {
                    self.media.foreign_jump_until = now + FOREIGN_JUMP_GRACE_MS;
                }
                self.media.pos_ms = ms;
                self.media.pos_at_ms = now;
            }
            PlayerEvent::Pause(paused) => {
                self.media.paused = paused;
                if self.media.file_loaded {
                    self.classify(RawEvent::Pause(paused)).await;
                }
            }
            PlayerEvent::Seeked { position_ms } => {
                self.media.foreign_jump_until = 0;
                self.media.pos_ms = position_ms;
                self.media.pos_at_ms = now;
                if self.media.file_loaded {
                    self.classify(RawEvent::Seeked { position_ms }).await;
                }
            }
            PlayerEvent::Speed(speed) => {
                self.media.speed = speed;
                if self.media.file_loaded {
                    self.classify(RawEvent::Speed(speed)).await;
                }
            }
            PlayerEvent::FileLoaded { duration_ms, title } => {
                self.media.file_loaded = true;
                self.media.duration_ms = duration_ms;
                self.media.title = title;
                self.media.pos_ms = 0;
                self.media.pos_at_ms = now;
                self.echo.load_finished();
                self.controller.reset_settle(now);
                self.update_status();
            }
            PlayerEvent::Buffering(buffering) => {
                self.media.buffering = buffering;
                if buffering {
                    if matches!(self.media.status, MediaStatus::Ready { .. }) {
                        self.set_status(MediaStatus::Stalled);
                    }
                } else {
                    self.update_status();
                }
            }
            PlayerEvent::CacheAhead { ms, eof } => {
                self.media.cache_ms = ms;
                self.media.cache_eof = eof;
                self.update_status();
            }
            PlayerEvent::Eof => {
                if let Some(item_id) = self.media.item {
                    self.send(ClientMsg::ItemEnded { item_id });
                }
            }
            PlayerEvent::LoadFailed { message } => {
                self.emit(UiEvent::Error(format!(
                    "The player could not open the media: {message}"
                )));
                self.set_status(MediaStatus::Failed { message });
            }
            PlayerEvent::Exited { code } => self.last_exit_code = Some(code),
        }
    }

    fn observed_position(&self, now: u64) -> u64 {
        let m = &self.media;
        if m.paused || m.buffering {
            m.pos_ms
        } else {
            m.pos_ms + (now.saturating_sub(m.pos_at_ms) as f64 * m.speed).round() as u64
        }
    }

    async fn classify(&mut self, ev: RawEvent) {
        let now = self.local_ms();
        let position_ms = self.observed_position(now);
        match self.echo.observe(now, ev, position_ms) {
            Classified::Ours | Classified::Ignored => {}
            Classified::User(action) => {
                let req =
                    match action {
                        UserAction::Pause { position_ms } => self.request(|req_id| {
                            ClientMsg::Pause { req_id, position_ms: Some(position_ms) }
                        }),
                        UserAction::Play { position_ms } => self.request(|req_id| {
                            ClientMsg::Play { req_id, position_ms: Some(position_ms) }
                        }),
                        UserAction::Seek { position_ms } => {
                            self.request(|req_id| ClientMsg::Seek { req_id, position_ms })
                        }
                    };
                self.awaiting = Some((req, now + AWAIT_REPLY_MS));
            }
            Classified::RevertSpeed => {
                let room_speed = self.snap.playback.as_ref().map_or(1.0, |p| p.speed);
                let speed = self.controller.desired_speed(room_speed);
                self.echo.issued(now, PendingOp::Speed(speed));
                if let Some(p) = &mut self.player {
                    let _ = p.set_speed(speed).await;
                }
            }
        }
    }

    // ---- sync tick ---------------------------------------------------------------------------

    async fn on_tick(&mut self) {
        let now = self.local_ms();
        if self.awaiting.is_some_and(|(_, deadline)| now > deadline) {
            self.awaiting = None;
        }
        if self.awaiting.is_some()
            || !self.media.file_loaded
            || self.player.is_none()
            || now < self.media.foreign_jump_until
            // The player cannot move while buffering; long stalls are the server's business.
            || self.media.buffering
        {
            return;
        }
        let Some(pb) = self.snap.playback.clone() else { return };
        if pb.item_id != self.media.item {
            return;
        }
        let Some(now_server) =
            self.conn.as_ref().and_then(|c| c.clock.lock().expect("clock").to_server_ms(now))
        else {
            return;
        };
        let target = Target {
            position_ms: pb.expected_position_ms(now_server),
            playing: pb.is_playing(),
            room_speed: pb.speed,
        };
        let observed = Observed {
            position_ms: self.observed_position(now),
            paused: self.media.paused,
            speed: self.media.speed,
        };
        for c in self.controller.update(now, &target, &observed) {
            self.apply_correction(now, c, &pb).await;
        }
    }

    async fn apply_correction(&mut self, now: u64, c: Correction, _pb: &PlaybackState) {
        let Some(p) = &mut self.player else { return };
        let result = match c {
            Correction::SetSpeed(speed) => {
                self.echo.issued(now, PendingOp::Speed(speed));
                p.set_speed(speed).await
            }
            Correction::Seek { position_ms } => {
                self.echo.issued(now, PendingOp::Seek { target_ms: position_ms });
                p.seek(position_ms).await
            }
            Correction::Pause => {
                self.echo.issued(now, PendingOp::Pause(true));
                p.pause().await
            }
            Correction::Play => {
                self.echo.issued(now, PendingOp::Pause(false));
                p.play().await
            }
        };
        if let Err(e) = result {
            tracing::debug!("player command failed: {e}");
        }
    }
}
