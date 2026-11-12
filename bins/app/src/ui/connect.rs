//! Address / nickname / mode and the connection state.

use sppy_client_core::api::{ConnState, Snapshot, UiCommand};
use sppy_client_core::config::DeliveryMode;

use crate::app::SppyApp;

pub fn state_text(c: &ConnState) -> String {
    match c {
        ConnState::Idle => "Not connected".into(),
        ConnState::Connecting => "Connecting…".into(),
        ConnState::Connected => "Connected".into(),
        ConnState::Reconnecting { attempt } => {
            format!("Server unavailable, reconnecting (attempt {attempt})")
        }
        ConnState::Rejected(code) => format!("Rejected: {code:?}"),
    }
}

pub fn show(app: &mut SppyApp, ui: &mut egui::Ui, snap: &Snapshot) {
    let connected =
        matches!(snap.connection, ConnState::Connected | ConnState::Reconnecting { .. });
    ui.add_space(4.0);
    ui.add_enabled_ui(!connected, |ui| {
        egui::Grid::new("connect-grid").num_columns(2).show(ui, |ui| {
            ui.label("Invite link");
            ui.add(
                egui::TextEdit::singleline(&mut app.ui.address).hint_text("sppy://host:8740/token"),
            );
            ui.end_row();
            ui.label("Nickname");
            ui.text_edit_singleline(&mut app.ui.nickname);
            ui.end_row();
        });
    });
    ui.horizontal(|ui| {
        let mut mode = snap.local.mode;
        ui.radio_value(&mut mode, DeliveryMode::Stream, "Stream");
        ui.radio_value(&mut mode, DeliveryMode::Preload, "Preload");
        if mode != snap.local.mode {
            app.config.mode = mode;
            app.save_config();
            app.send(UiCommand::SetMode(mode));
        }
        ui.separator();
        if connected {
            if ui.button("Disconnect").clicked() {
                app.send(UiCommand::Disconnect);
            }
        } else {
            let can = !app.ui.address.trim().is_empty() && !app.ui.nickname.trim().is_empty();
            if ui.add_enabled(can, egui::Button::new("Connect")).clicked() {
                let address = app.ui.address.trim().to_owned();
                app.remember_server(&address);
                app.send(UiCommand::Connect {
                    address,
                    nickname: app.ui.nickname.trim().to_owned(),
                });
            }
        }
        if !connected && app.host.is_none() {
            let can = !app.ui.nickname.trim().is_empty() && !app.host_starting();
            if ui.add_enabled(can, egui::Button::new("Become host")).clicked() {
                app.become_host();
            }
        }
        if !snap.player_open && connected && ui.button("Open player").clicked() {
            app.send(UiCommand::OpenPlayer);
        }
    });
    ui.label(state_text(&snap.connection));
    if let Some(host) = &app.host {
        ui.horizontal(|ui| {
            ui.label(format!("Hosting on port {}", host.local_addr.port()));
            if let Some(link) = snap.invite_token.as_deref().and_then(|t| app.invite_link(t))
                && ui.button("Copy invite link").on_hover_text(&link).clicked()
            {
                ui.ctx().copy_text(link);
            }
        });
    }
    ui.add_space(4.0);
}
