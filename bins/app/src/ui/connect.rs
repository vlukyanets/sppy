//! Connection: a form when disconnected, one compact line when connected.

use sppy_client_core::api::{ConnState, Snapshot, UiCommand};
use sppy_client_core::config::DeliveryMode;
use sppy_client_core::netaddr;

use super::theme;
use crate::app::SppyApp;

pub fn state_text(c: &ConnState) -> String {
    match c {
        ConnState::Idle => "Not connected".into(),
        ConnState::Connecting => "Connecting…".into(),
        ConnState::Connected => "Connected".into(),
        ConnState::Reconnecting { attempt } => {
            format!("Server unavailable, reconnecting (attempt {attempt})")
        }
        ConnState::Rejected(code @ sppy_proto::ErrorCode::VersionMismatch { .. }) => {
            sppy_client_core::notice::reject_text(code, "")
        }
        ConnState::Rejected(code) => format!("Rejected: {code:?}"),
    }
}

/// Whether this link would carry the room over the internet unencrypted.
fn unencrypted(link: &str) -> bool {
    sppy_common::link::parse_address(link).is_ok_and(|a| a.unencrypted_over_internet())
}

const UNENCRYPTED: &str = "Not encrypted: the invite token, chat and video cross the internet \
    readable by anyone on the way. Use an sppys:// link (the server behind a TLS proxy, see \
    the README) or a VPN such as Tailscale.";

fn unencrypted_badge(ui: &mut egui::Ui) {
    ui.colored_label(super::YELLOW, "⚠ Not encrypted").on_hover_text(UNENCRYPTED);
}

/// How library files reach me. Not shown while I host: my library is my own folders, so
/// its files always play from my disk (and links always stream).
fn mode_switch(app: &mut SppyApp, ui: &mut egui::Ui, snap: &Snapshot) {
    if app.host.is_some() {
        return;
    }
    let mut mode = snap.local.mode;
    theme::segmented(
        ui,
        &mut mode,
        &[(DeliveryMode::Stream, "Stream"), (DeliveryMode::Preload, "Preload")],
    )
    .on_hover_text(
        "Stream: play files directly from the server.\nPreload: download each file before playing it.",
    );
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
    if online {
        ui.horizontal(|ui| {
            let color =
                if snap.connection == ConnState::Connected { super::GREEN } else { super::YELLOW };
            ui.colored_label(color, "●");
            ui.label(egui::RichText::new(&app.ui.nickname).strong());
            ui.label(egui::RichText::new(state_text(&snap.connection)).color(theme::MUTED));
            if app.host.is_none() && unencrypted(&app.ui.address) {
                unencrypted_badge(ui);
            }
            ui.add_space(10.0);
            mode_switch(app, ui, snap);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("⚙").on_hover_text("Settings").clicked() {
                    app.ui.settings_open = !app.ui.settings_open;
                }
                if app.host.is_some() {
                    let others = snap.room.as_ref().is_some_and(|r| {
                        r.participants.iter().any(|p| p.connected && Some(p.id) != snap.me)
                    });
                    if ui
                        .button("Stop hosting")
                        .on_hover_text("Leave the room and stop your server")
                        .clicked()
                    {
                        if others {
                            app.ui.confirm_stop_hosting = true;
                        } else {
                            app.stop_hosting();
                        }
                    }
                } else if ui.button("Disconnect").clicked() {
                    app.send(UiCommand::Disconnect);
                }
                if !snap.player_open && ui.add(theme::primary("Open player")).clicked() {
                    app.send(UiCommand::OpenPlayer);
                }
                if let Some(host) = &app.host {
                    let port = host.local_addr.port();
                    ui.add_space(10.0);
                    if let Some(link) =
                        snap.invite_token.as_deref().and_then(|t| app.invite_link(t))
                    {
                        if unencrypted(&link) {
                            unencrypted_badge(ui);
                        }
                        if ui.add(theme::primary("Copy invite link")).on_hover_text(&link).clicked()
                        {
                            ui.ctx().copy_text(link.clone());
                        }
                    }
                    invite_host_choice(app, ui);
                    ui.label(
                        egui::RichText::new(format!("Hosting on :{port}")).color(theme::MUTED),
                    );
                }
            });
        });
    }
}

/// When not in a room: the join form, as a card in the middle of the window.
pub fn join_card(app: &mut SppyApp, ui: &mut egui::Ui, snap: &Snapshot) {
    ui.add_space((ui.available_height() * 0.18).max(20.0));
    ui.vertical_centered(|ui| {
        ui.label(egui::RichText::new("sppy").size(30.0).strong().color(theme::ACCENT));
        ui.label(egui::RichText::new("Watch together, in sync").color(theme::MUTED));
        ui.add_space(14.0);
    });
    let width = 480.0_f32.min(ui.available_width() - 40.0);
    ui.vertical_centered(|ui| {
        egui::Frame::new()
            .fill(ui.visuals().faint_bg_color)
            .corner_radius(egui::CornerRadius::same(12))
            .inner_margin(egui::Margin::same(20))
            .show(ui, |ui| {
                ui.set_width(width);
                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                    join_form(app, ui, snap)
                });
            });
    });
}

fn join_form(app: &mut SppyApp, ui: &mut egui::Ui, snap: &Snapshot) {
    ui.set_width(ui.available_width());
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Join a room").heading().strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("⚙").on_hover_text("Settings").clicked() {
                app.ui.settings_open = !app.ui.settings_open;
            }
        });
    });
    ui.add_space(4.0);
    let field = |ui: &mut egui::Ui, label: &str, text: &mut String, hint: &str| {
        ui.label(egui::RichText::new(label).small().color(theme::MUTED));
        ui.add(
            egui::TextEdit::singleline(text)
                .hint_text(hint)
                .margin(egui::Margin::symmetric(8, 6))
                .desired_width(f32::INFINITY),
        );
        ui.add_space(4.0);
    };
    field(ui, "Invite link", &mut app.ui.address, "sppy://host:8740/token");
    field(ui, "Nickname", &mut app.ui.nickname, "How others see you");
    if unencrypted(&app.ui.address) {
        ui.add(egui::Label::new(egui::RichText::new(UNENCRYPTED).color(super::YELLOW)).wrap());
    }
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let has_nick = !app.ui.nickname.trim().is_empty();
        let can_join = has_nick && !app.ui.address.trim().is_empty();
        let hosting = app.host.is_some();
        let connect =
            if hosting { egui::Button::new("Connect") } else { theme::primary("Connect") };
        if ui.add_enabled(can_join, connect).clicked() {
            let address = app.ui.address.trim().to_owned();
            app.remember_server(&address);
            app.send(UiCommand::Connect { address, nickname: app.ui.nickname.trim().to_owned() });
        }
        if hosting {
            if ui
                .add(theme::primary("Back to my room"))
                .on_hover_text("Your server is still running")
                .clicked()
            {
                app.rejoin_own_room();
            }
            if ui
                .button("Stop hosting")
                .on_hover_text("Everyone in your room is disconnected")
                .clicked()
            {
                app.stop_hosting();
            }
        } else if ui
            .add_enabled(has_nick && !app.host_starting(), egui::Button::new("Become host"))
            .on_hover_text("Run the server here and invite others")
            .clicked()
        {
            app.become_host();
        }
    });
    if app.host.is_none() {
        ui.add_space(6.0);
        ui.horizontal(|ui| mode_switch(app, ui, snap));
    }
    if !matches!(snap.connection, ConnState::Idle) {
        ui.label(egui::RichText::new(state_text(&snap.connection)).color(theme::MUTED));
    }
    ui.add_space(6.0);
}

/// Confirmation before stopping the server while others are watching.
pub fn stop_hosting_dialog(app: &mut SppyApp, ctx: &egui::Context) {
    if !app.ui.confirm_stop_hosting {
        return;
    }
    egui::Window::new("Stop hosting")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label("Others are in your room. Stopping the server disconnects everyone.");
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.add(theme::primary("Stop hosting")).clicked() {
                    app.ui.confirm_stop_hosting = false;
                    app.stop_hosting();
                }
                if ui.button("Cancel").clicked() {
                    app.ui.confirm_stop_hosting = false;
                }
            });
        });
}
