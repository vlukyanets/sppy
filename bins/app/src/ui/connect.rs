//! Connection: a form when disconnected, one compact line when connected.

use sppy_client_core::api::{ConnState, Snapshot, UiCommand};
use sppy_client_core::config::DeliveryMode;
use sppy_client_core::netaddr;

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

/// Which address goes into the invite link: one of this computer's, or one typed in.
fn invite_host_choice(app: &mut SppyApp, ui: &mut egui::Ui) {
    if app.ui.custom_host.is_some() {
        custom_host_edit(app, ui);
        return;
    }
    let (options, chosen) = app.invite_hosts();
    let other = options.len();
    let mut picked = chosen;
    let current = options.get(chosen).map(|o| o.label()).unwrap_or_default();
    egui::ComboBox::from_id_salt("invite-host")
        .selected_text(current)
        .show_ui(ui, |ui| {
            for (i, o) in options.iter().enumerate() {
                ui.selectable_value(&mut picked, i, o.label());
            }
            ui.separator();
            ui.selectable_value(&mut picked, other, "Other address…");
        })
        .response
        .on_hover_text("Address put into the invite link: pick the one others can reach");
    if picked == other {
        app.ui.custom_host = Some(app.config.host.public_address.clone().unwrap_or_default());
    } else if picked != chosen {
        app.config.invite_host = Some(options[picked].key());
        app.save_config();
    }
}

/// Text field for an address others reach this computer at (saved as the public address).
fn custom_host_edit(app: &mut SppyApp, ui: &mut egui::Ui) {
    let Some(text) = app.ui.custom_host.as_mut() else { return };
    let edit = ui
        .add(egui::TextEdit::singleline(text).hint_text("host or host:port").desired_width(200.0));
    if !edit.has_focus() && !edit.lost_focus() {
        edit.request_focus();
    }
    let parsed = netaddr::parse_custom_host(text);
    let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    let ok = ui.add_enabled(parsed.is_ok(), egui::Button::new("OK")).clicked();
    let cancel = ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape));
    if let Err(e) = &parsed
        && !text.trim().is_empty()
    {
        ui.colored_label(super::RED, *e);
    }
    if cancel {
        app.ui.custom_host = None;
    } else if (ok || enter)
        && let Ok(host) = parsed
    {
        app.config.host.public_address = Some(host.clone());
        app.config.invite_host = Some(host);
        app.save_config();
        app.ui.custom_host = None;
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
                invite_host_choice(app, ui);
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
