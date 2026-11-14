//! Connection: a form when disconnected, one compact line when connected.

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

fn mode_switch(app: &mut SppyApp, ui: &mut egui::Ui, snap: &Snapshot) {
    let mut mode = snap.local.mode;
    ui.radio_value(&mut mode, DeliveryMode::Stream, "Stream")
        .on_hover_text("Play files directly from the server");
    ui.radio_value(&mut mode, DeliveryMode::Preload, "Preload")
        .on_hover_text("Download each file before playing it");
    if mode != snap.local.mode {
        app.config.mode = mode;
        app.save_config();
        app.send(UiCommand::SetMode(mode));
    }
}

pub fn show(app: &mut SppyApp, ui: &mut egui::Ui, snap: &Snapshot) {
    let online = matches!(snap.connection, ConnState::Connected | ConnState::Reconnecting { .. });
    ui.add_space(4.0);
    if online {
        ui.horizontal_wrapped(|ui| {
            let color =
                if snap.connection == ConnState::Connected { super::GREEN } else { super::YELLOW };
            ui.colored_label(color, "●");
            ui.label(format!("{} as {}", state_text(&snap.connection), app.ui.nickname));
            ui.separator();
            mode_switch(app, ui, snap);
            ui.separator();
            if let Some(host) = &app.host {
                ui.label(format!("Hosting on port {}", host.local_addr.port()));
                if let Some(link) = snap.invite_token.as_deref().and_then(|t| app.invite_link(t))
                    && ui.button("Copy invite link").on_hover_text(&link).clicked()
                {
                    ui.ctx().copy_text(link);
                }
                ui.separator();
            }
            if !snap.player_open && ui.button("Open player").clicked() {
                app.send(UiCommand::OpenPlayer);
            }
            if ui.button("Disconnect").clicked() {
                app.send(UiCommand::Disconnect);
            }
            if ui.button("⚙").on_hover_text("Settings").clicked() {
                app.ui.settings_open = !app.ui.settings_open;
            }
        });
        ui.add_space(4.0);
        return;
    }
    egui::Grid::new("connect-grid").num_columns(2).show(ui, |ui| {
        ui.label("Invite link");
        ui.add(
            egui::TextEdit::singleline(&mut app.ui.address)
                .hint_text("sppy://host:8740/token")
                .desired_width(320.0),
        );
        ui.end_row();
        ui.label("Nickname");
        ui.text_edit_singleline(&mut app.ui.nickname);
        ui.end_row();
    });
    ui.horizontal(|ui| {
        mode_switch(app, ui, snap);
        ui.separator();
        let has_nick = !app.ui.nickname.trim().is_empty();
        let can_join = has_nick && !app.ui.address.trim().is_empty();
        if ui.add_enabled(can_join, egui::Button::new("Connect")).clicked() {
            let address = app.ui.address.trim().to_owned();
            app.remember_server(&address);
            app.send(UiCommand::Connect { address, nickname: app.ui.nickname.trim().to_owned() });
        }
        if app.host.is_none()
            && ui
                .add_enabled(has_nick && !app.host_starting(), egui::Button::new("Become host"))
                .on_hover_text("Run the server here and invite others")
                .clicked()
        {
            app.become_host();
        }
        if ui.button("⚙").on_hover_text("Settings").clicked() {
            app.ui.settings_open = !app.ui.settings_open;
        }
    });
    if !matches!(snap.connection, ConnState::Idle) {
        ui.label(state_text(&snap.connection));
    }
    ui.add_space(4.0);
}
