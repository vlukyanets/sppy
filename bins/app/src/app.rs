//! Application state and the top-level layout.

use sppy_client_core::api::{ClientHandle, Snapshot, UiCommand, UiEvent};
use std::time::{Duration, Instant};

use sppy_client_core::config::{ClientConfig, ClientPaths, SavedServer};
use sppy_client_core::netaddr::{self, InviteHost, LocalIp};
use sppy_player::any::{AnyLauncher, PlayerSetup};
use sppy_server::ServerHandle;
use tokio::runtime::Runtime;
use tokio::sync::broadcast;

use crate::log::EventLog;
pub use crate::log::LogKind;
use crate::ui;

const LOG_LINES: usize = 300;
const LOCAL_IPS_REFRESH: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Playlist,
    Library,
}

/// Purely visual state kept between frames.
pub struct UiState {
    pub address: String,
    pub nickname: String,
    pub log: EventLog,
    pub tab: Tab,
    /// Show notices (pauses, joins, …) in the chat panel.
    pub show_events: bool,
    pub library_query: String,
    pub last_error: Option<String>,
    pub url: String,
    pub library_requested: bool,
    pub chat: String,
    pub chat_seeded_for: Option<sppy_proto::ParticipantId>,
    pub settings_open: bool,
    pub extra_args: String,
    pub audio_languages: String,
    pub subtitle_languages: String,
    pub confirm_kick: Option<(sppy_proto::ParticipantId, String)>,
    /// "Stop hosting" was clicked while others are in the room.
    pub confirm_stop_hosting: bool,
    /// The player could not start (shown as a banner with a Settings button).
    pub player_problem: Option<String>,
    /// This machine's addresses, refreshed while hosting or in the settings.
    pub local_ips: Vec<LocalIp>,
    local_ips_at: Option<Instant>,
    /// "Add folder…" was refused.
    pub folder_error: Option<String>,
    /// A server folder about to stop being shared (asks first).
    pub confirm_unshare: Option<String>,
    pub server_folder_path: String,
    pub server_folder_name: String,
    /// The invite address being typed ("Other address…").
    pub custom_host: Option<String>,
    /// Folders the core searches for local copies, as last sent to it.
    pub media_folders: Vec<std::path::PathBuf>,
    /// An invite link that arrived while in a room (it is also in `address`).
    pub invite_received: Option<String>,
    /// Whether this app opens `sppy://` links: looked up when Settings open.
    pub link_handler: Option<Option<bool>>,
    pub link_handler_error: Option<String>,
}

pub struct SppyApp {
    /// Runs the client core and the embedded server; must outlive the window.
    pub rt: Runtime,
    pub handle: ClientHandle,
    /// Shared with the client core: changing it affects the next player launch.
    pub launcher: AnyLauncher,
    /// What the launcher starts now; Save restarts the player when the settings differ.
    pub player_setup: PlayerSetup,
    pub config: ClientConfig,
    pub paths: ClientPaths,
    pub ui: UiState,
    events: broadcast::Receiver<UiEvent>,
    /// The embedded server while hosting.
    pub host: Option<ServerHandle>,
    host_starting: Option<std::sync::mpsc::Receiver<Result<ServerHandle, String>>>,
    /// Invite links handed over by later starts of the app.
    links: std::sync::mpsc::Receiver<String>,
}

impl SppyApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        rt: Runtime,
        handle: ClientHandle,
        launcher: AnyLauncher,
        config: ClientConfig,
        paths: ClientPaths,
        link: Option<String>,
    ) -> Self {
        ui::theme::apply(&cc.egui_ctx);
        let (link_tx, links) = std::sync::mpsc::channel();
        let link_ctx = cc.egui_ctx.clone();
        crate::instance::listen(&rt, &paths.data_dir, move |link| {
            let _ = link_tx.send(link);
            link_ctx.request_repaint();
        });
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
            address: link.unwrap_or_else(|| {
                config.servers.last().map(|s| s.address.clone()).unwrap_or_default()
            }),
            nickname: config.nickname.clone(),
            log: EventLog::new(LOG_LINES),
            tab: Tab::Playlist,
            show_events: true,
            library_query: String::new(),
            last_error: None,
            url: String::new(),
            library_requested: false,
            chat: String::new(),
            chat_seeded_for: None,
            settings_open: false,
            extra_args: config.player.extra_args.join(" "),
            audio_languages: config.audio_languages.join(", "),
            subtitle_languages: config.subtitle_languages.join(", "),
            confirm_kick: None,
            confirm_stop_hosting: false,
            player_problem: None,
            local_ips: Vec::new(),
            local_ips_at: None,
            custom_host: None,
            folder_error: None,
            confirm_unshare: None,
            server_folder_path: String::new(),
            server_folder_name: String::new(),
            media_folders: config.media_folders(),
            invite_received: None,
            link_handler: None,
            link_handler_error: None,
        };
        let events = handle.subscribe_events();
        SppyApp {
            rt,
            handle,
            launcher,
            player_setup: config.player.setup(),
            config,
            paths,
            ui,
            events,
            host: None,
            host_starting: None,
            links,
        }
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
        self.ui.log.push(kind, text);
    }

    /// Apply the player settings and restart the player with them.
    pub fn restart_player(&mut self) {
        self.player_setup = self.config.player.setup();
        self.launcher.set_setup(self.player_setup.clone());
        self.ui.player_problem = None;
        self.send(UiCommand::RestartPlayer);
    }

    /// Remember the nickname and server for next time.
    pub fn remember_server(&mut self, address: &str) {
        self.config.nickname = self.ui.nickname.clone();
        self.config.servers.retain(|s| s.address != address);
        self.config.servers.push(SavedServer { address: address.to_owned() });
        self.save_config();
    }

    /// Links to `site` play without asking from now on.
    pub fn trust_site(&mut self, site: &str) {
        if !self.config.trusted_sites.iter().any(|s| s.eq_ignore_ascii_case(site)) {
            self.config.trusted_sites.push(site.to_owned());
            self.save_config();
        }
        self.send(UiCommand::SetTrustedSites(self.config.trusted_sites.clone()));
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

    /// A clicked invite link reached this window: put it into the join form, never join
    /// on its own.
    fn take_links(&mut self, ctx: &egui::Context) {
        while let Ok(link) = self.links.try_recv() {
            let online = matches!(
                self.snapshot().connection,
                sppy_client_core::api::ConnState::Connected
                    | sppy_client_core::api::ConnState::Reconnecting { .. }
            );
            let host = sppy_common::link::parse_address(&link).map(|a| a.host).unwrap_or_default();
            tracing::info!("invite link to {host} received from another start of the app");
            self.ui.address = link.clone();
            self.ui.invite_received = online.then_some(link);
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
    }

    /// Join the room of the server running here again (after Disconnect).
    pub fn rejoin_own_room(&mut self) {
        let Some(host) = &self.host else { return };
        let cmd = UiCommand::BecomeHostConnect {
            listen: host.local_addr,
            host_token: host.host_token.clone(),
        };
        self.send(cmd);
    }

    /// Leave the room and stop the server running here; whoever is in it is disconnected.
    pub fn stop_hosting(&mut self) {
        self.send(UiCommand::Disconnect);
        if let Some(host) = self.host.take() {
            self.rt.spawn(host.shutdown());
        }
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

    /// Hosts the invite link can use, and the index of the chosen one.
    pub fn invite_hosts(&self) -> (Vec<InviteHost>, usize) {
        let reachable = match &self.host {
            Some(host) => netaddr::reachable_via(&self.ui.local_ips, host.local_addr),
            None => Vec::new(),
        };
        let options =
            netaddr::invite_options(self.config.host.public_address.as_deref(), &reachable);
        let chosen = netaddr::pick_option(&options, self.config.invite_host.as_deref());
        (options, chosen)
    }

    /// Invite link for the embedded server.
    pub fn invite_link(&self, token: &str) -> Option<String> {
        let port = self.host.as_ref()?.local_addr.port();
        let (options, chosen) = self.invite_hosts();
        Some(match options.get(chosen) {
            Some(host) => host.link(port, token),
            None => netaddr::invite_link(None, port, token, None),
        })
    }

    /// Re-read the local addresses now and then: Wi-Fi or a VPN may come and go.
    fn refresh_local_ips(&mut self) {
        let needed = self.host.is_some() || self.ui.settings_open;
        if !needed || self.ui.local_ips_at.is_some_and(|t| t.elapsed() < LOCAL_IPS_REFRESH) {
            return;
        }
        self.ui.local_ips = netaddr::local_ips();
        self.ui.local_ips_at = Some(Instant::now());
    }

    /// Tell the core about changed media folders (it rescans them); `force` rescans anyway.
    pub fn apply_media_folders(&mut self, force: bool) {
        let folders = self.config.media_folders();
        if force || folders != self.ui.media_folders {
            self.ui.media_folders = folders.clone();
            self.send(UiCommand::SetMediaFolders(folders));
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
                Ok(UiEvent::PlayerUnavailable(t)) => {
                    self.ui.player_problem = Some(t.clone());
                    self.push_log(LogKind::Error, t);
                }
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    }
}

impl eframe::App for SppyApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_events();
        self.take_links(ctx);
        self.poll_host();
        self.refresh_local_ips();
        if self.host_starting.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
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
        let ctx = ui.ctx().clone();
        let online = matches!(
            snap.connection,
            sppy_client_core::api::ConnState::Connected
                | sppy_client_core::api::ConnState::Reconnecting { .. }
        );
        if online {
            let top = ui::theme::panel_frame(ui, egui::Margin::symmetric(14, 8));
            egui::Panel::top("connect")
                .frame(top)
                .show(ui, |ui| ui::connect::show(self, ui, &snap));
        }
        let bottom = ui::theme::panel_frame(ui, egui::Margin::symmetric(14, 4));
        egui::Panel::bottom("status").frame(bottom).show(ui, |ui| ui::status_line(self, ui, &snap));
        ui::settings::show(self, &ctx, &snap);
        ui::participants::kick_dialog(self, &ctx);
        ui::connect::stop_hosting_dialog(self, &ctx);
        if snap.room.is_none() {
            self.ui.library_requested = false;
            let content = ui::theme::content_frame(ui);
            egui::CentralPanel::default().frame(content).show(ui, |ui| {
                ui::banners(self, ui, &snap);
                if !online {
                    ui::connect::join_card(self, ui, &snap);
                }
            });
            return;
        }
        let side = ui::theme::panel_frame(ui, egui::Margin::symmetric(14, 10));
        egui::Panel::left("people")
            .frame(side)
            .resizable(true)
            .default_size(300.0)
            .min_size(240.0)
            .max_size(460.0)
            .show(ui, |ui| {
                ui::theme::section(ui, "Participants");
                ui::participants::show(self, ui, &snap);
                if snap.is_host {
                    ui.add_space(6.0);
                    ui::host::show(self, ui, &snap);
                }
                ui.add_space(8.0);
                ui::chat::show(self, ui, &snap);
            });
        let content = ui::theme::content_frame(ui);
        egui::CentralPanel::default().frame(content).show(ui, |ui| {
            ui::banners(self, ui, &snap);
            ui::playlist::now_playing(self, ui, &snap);
            ui.add_space(10.0);
            ui::theme::segmented(
                ui,
                &mut self.ui.tab,
                &[(Tab::Playlist, "Playlist"), (Tab::Library, "Library")],
            );
            ui.add_space(6.0);
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                match self.ui.tab {
                    Tab::Playlist => ui::playlist::show(self, ui, &snap),
                    Tab::Library => ui::library::show(self, ui, &snap),
                }
            });
        });
    }
}
