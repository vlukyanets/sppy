//! Application state and the top-level layout.

use std::collections::VecDeque;

use sppy_client_core::api::{ClientHandle, Snapshot, UiCommand, UiEvent};
use sppy_client_core::config::{ClientConfig, ClientPaths, SavedServer};
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
}

pub struct SppyApp {
    /// Runs the client core (and the embedded server); must outlive the window.
    #[allow(dead_code)]
    pub rt: Runtime,
    pub handle: ClientHandle,
    pub config: ClientConfig,
    pub paths: ClientPaths,
    pub ui: UiState,
    events: broadcast::Receiver<UiEvent>,
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
        };
        let events = handle.subscribe_events();
        SppyApp { rt, handle, config, paths, ui, events }
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
    fn logic(&mut self, _ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_events();
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let snap = self.snapshot();
        egui::Panel::top("connect").show(ui, |ui| ui::connect::show(self, ui, &snap));
        egui::Panel::bottom("status").show(ui, |ui| ui::status_line(self, ui, &snap));
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui::participants::show(self, ui, &snap);
            });
        });
    }
}
