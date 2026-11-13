//! Application state and the top-level layout.

use std::collections::VecDeque;

use sppy_client_core::api::{ClientHandle, Snapshot, UiCommand, UiEvent};
use sppy_client_core::config::{ClientConfig, ClientPaths, SavedServer};
use sppy_server::ServerHandle;
use tokio::runtime::Runtime;
use tokio::sync::broadcast;

use crate::ui;

const LOG_LINES: usize = 200;

#[derive(Debug, Clone, PartialEq)]
pub enum LogKind {
    Chat,
    Notice,
    Error,
}

/// Purely visual state kept between frames.
pub struct UiState {
    pub address: String,
    pub nickname: String,
    pub log: VecDeque<(LogKind, String)>,
    pub last_error: Option<String>,
    pub url: String,
    pub library_requested: bool,
    pub chat: String,
    pub chat_seeded_for: Option<sppy_proto::ParticipantId>,
    pub settings_open: bool,
    pub extra_args: String,
    pub subtitle_languages: String,
    pub confirm_kick: Option<(sppy_proto::ParticipantId, String)>,
}

pub struct SppyApp {
    /// Runs the client core and the embedded server; must outlive the window.
    pub rt: Runtime,
    pub handle: ClientHandle,
    pub config: ClientConfig,
    pub paths: ClientPaths,
    pub ui: UiState,
    events: broadcast::Receiver<UiEvent>,
    /// The embedded server while hosting.
    pub host: Option<ServerHandle>,
    host_starting: Option<std::sync::mpsc::Receiver<Result<ServerHandle, String>>>,
}

impl SppyApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        rt: Runtime,
        handle: ClientHandle,
        config: ClientConfig,
        paths: ClientPaths,
    ) -> Self {
        // Repaint whenever the core publishes a new snapshot or event.
        let ctx = cc.egui_ctx.clone();
        let mut state = handle.state.clone();
        let mut events = handle.subscribe_events();
        rt.spawn(async move {
            loop {
                tokio::select! {
                    r = state.changed() => if r.is_err() { return },
                    r = events.recv() => if matches!(r, Err(broadcast::error::RecvError::Closed)) { return },
                }
                ctx.request_repaint();
            }
        });
        let ui = UiState {
            address: config.servers.last().map(|s| s.address.clone()).unwrap_or_default(),
            nickname: config.nickname.clone(),
            log: VecDeque::new(),
            last_error: None,
            url: String::new(),
            library_requested: false,
            chat: String::new(),
            chat_seeded_for: None,
            settings_open: false,
            extra_args: config.player.extra_args.join(" "),
            subtitle_languages: config.subtitle_languages.join(", "),
            confirm_kick: None,
        };
        let events = handle.subscribe_events();
        SppyApp { rt, handle, config, paths, ui, events, host: None, host_starting: None }
    }

    pub fn snapshot(&self) -> Snapshot {
        self.handle.state.borrow().clone()
    }

    pub fn send(&self, cmd: UiCommand) {
        let _ = self.handle.commands.send(cmd);
    }

    pub fn push_log(&mut self, kind: LogKind, text: String) {
        if kind == LogKind::Error {
            self.ui.last_error = Some(text.clone());
        }
        self.ui.log.push_back((kind, text));
        while self.ui.log.len() > LOG_LINES {
            self.ui.log.pop_front();
        }
    }

    /// Remember the nickname and server for next time.
    pub fn remember_server(&mut self, address: &str) {
        self.config.nickname = self.ui.nickname.clone();
        self.config.servers.retain(|s| s.address != address);
        self.config.servers.push(SavedServer { address: address.to_owned() });
        self.save_config();
    }

    pub fn save_config(&mut self) {
        if let Err(e) = self.config.save(&self.paths.config_file) {
            self.push_log(LogKind::Error, e.to_string());
        }
    }

    /// Start the embedded server; the client joins it once it is up.
    pub fn become_host(&mut self) {
        if self.host.is_some() || self.host_starting.is_some() {
            return;
        }
        self.config.nickname = self.ui.nickname.trim().to_owned();
        self.save_config();
        let (tx, rx) = std::sync::mpsc::channel();
        let (cfg, dir) = (self.config.host.clone(), self.paths.host_data_dir.clone());
        self.rt.spawn(async move {
            let _ = tx.send(crate::host::become_host(cfg, dir).await.map_err(|e| e.to_string()));
        });
        self.host_starting = Some(rx);
    }

    pub fn host_starting(&self) -> bool {
        self.host_starting.is_some()
    }

    fn poll_host(&mut self) {
        let Some(rx) = &self.host_starting else { return };
        match rx.try_recv() {
            Ok(Ok(handle)) => {
                self.host_starting = None;
                self.send(UiCommand::BecomeHostConnect {
                    listen: handle.local_addr,
                    host_token: handle.host_token.clone(),
                });
                self.host = Some(handle);
            }
            Ok(Err(e)) => {
                self.host_starting = None;
                self.push_log(LogKind::Error, format!("Cannot start the server: {e}"));
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => self.host_starting = None,
        }
    }

    /// Invite link for the embedded server.
    pub fn invite_link(&self, token: &str) -> Option<String> {
        let port = self.host.as_ref()?.local_addr.port();
        Some(sppy_client_core::netaddr::invite_link(
            self.config.host.public_address.as_deref(),
            port,
            token,
            sppy_client_core::netaddr::first_non_loopback_ip(),
        ))
    }

    fn drain_events(&mut self) {
        loop {
            match self.events.try_recv() {
                Ok(UiEvent::Chat(c)) => {
                    self.push_log(LogKind::Chat, format!("{}: {}", c.nickname, c.text))
                }
                Ok(UiEvent::Notice(t)) => self.push_log(LogKind::Notice, t),
                Ok(UiEvent::Error(t)) => self.push_log(LogKind::Error, t),
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    }
}

impl eframe::App for SppyApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_events();
        self.poll_host();
        if self.host_starting.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    fn on_exit(&mut self) {
        let _ = self.handle.commands.send(UiCommand::Disconnect);
        if let Some(host) = self.host.take() {
            self.rt.block_on(host.shutdown());
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let snap = self.snapshot();
        egui::Panel::top("connect").show(ui, |ui| ui::connect::show(self, ui, &snap));
        egui::Panel::bottom("status").show(ui, |ui| ui::status_line(self, ui, &snap));
        let ctx = ui.ctx().clone();
        ui::settings::show(self, &ctx, &snap);
        ui::participants::kick_dialog(self, &ctx);
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                if snap.room.is_none() {
                    self.ui.library_requested = false;
                    return;
                }
                egui::CollapsingHeader::new("Participants")
                    .default_open(true)
                    .show(ui, |ui| ui::participants::show(self, ui, &snap));
                egui::CollapsingHeader::new("Playlist")
                    .default_open(true)
                    .show(ui, |ui| ui::playlist::show(self, ui, &snap));
                egui::CollapsingHeader::new("Library")
                    .show(ui, |ui| ui::library::show(self, ui, &snap));
                if snap.is_host {
                    egui::CollapsingHeader::new("Host")
                        .show(ui, |ui| ui::host::show(self, ui, &snap));
                }
                egui::CollapsingHeader::new("Chat")
                    .default_open(true)
                    .show(ui, |ui| ui::chat::show(self, ui, &snap));
            });
        });
    }
}
