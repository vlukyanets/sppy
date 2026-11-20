//! UI panels.

pub mod chat;
pub mod connect;
pub mod host;
pub mod library;
pub mod numbers;
pub mod participants;
pub mod playlist;
pub mod settings;
pub mod theme;

use egui::Color32;
use sppy_client_core::api::{Snapshot, UiCommand};
use sppy_client_core::notice::waiting_line;

use crate::app::SppyApp;

pub const GREEN: Color32 = Color32::from_rgb(98, 200, 140);
pub const YELLOW: Color32 = Color32::from_rgb(232, 186, 82);
pub const RED: Color32 = Color32::from_rgb(232, 96, 96);

/// "sppy 0.2.0 · protocol v3": both sides of a room need the same protocol.
pub fn version_text() -> String {
    format!("sppy {} · protocol v{}", env!("CARGO_PKG_VERSION"), sppy_proto::PROTOCOL_VERSION)
}

/// Last error (red, dismissable) or last notice on the left, the version on the right.
pub fn status_line(app: &mut SppyApp, ui: &mut egui::Ui, _snap: &Snapshot) {
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(egui::RichText::new(version_text()).small().color(theme::MUTED));
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                if let Some(err) = &app.ui.last_error {
                    ui.colored_label(ui.visuals().error_fg_color, err);
                    if theme::icon(ui, "✕", "Dismiss") {
                        app.ui.last_error = None;
                    }
                } else if let Some(e) = app.ui.log.last_notice() {
                    ui.label(egui::RichText::new(e.display()).small().color(theme::MUTED));
                }
            });
        });
    });
}

/// Things that need attention: the player could not start, a link to an unknown site,
/// the room is waiting, the host has no media folders.
pub fn banners(app: &mut SppyApp, ui: &mut egui::Ui, snap: &Snapshot) {
    if let Some(problem) = app.ui.player_problem.clone() {
        theme::callout(ui, RED, |ui| {
            ui.colored_label(RED, problem);
            ui.horizontal(|ui| {
                if ui.button("Open settings").clicked() {
                    app.ui.settings_open = true;
                }
                if ui.button("Try again").clicked() {
                    app.restart_player();
                }
            });
        });
    }
    if let Some(link) = app.ui.invite_received.clone() {
        let host = sppy_common::link::parse_address(&link).map(|a| a.host).unwrap_or_default();
        theme::callout(ui, theme::ACCENT, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(format!(
                    "An invite link to {host} is waiting in the join form: leave this room to use it."
                ));
                if ui.button("Dismiss").clicked() {
                    app.ui.invite_received = None;
                }
            });
        });
    }
    if let Some(q) = &snap.link_question {
        theme::callout(ui, YELLOW, |ui| {
            ui.label(format!(
                "{} added a link to {}. Your player will connect to that site.",
                q.added_by, q.site
            ));
            ui.horizontal_wrapped(|ui| {
                if ui.add(theme::primary("Play")).on_hover_text("Open this link once").clicked() {
                    app.send(UiCommand::AllowLink(q.item_id));
                }
                if ui
                    .button(format!("Always allow {}", q.site))
                    .on_hover_text("Open links to this site without asking (Settings)")
                    .clicked()
                {
                    app.trust_site(&q.site);
                }
                if ui.button("Skip").on_hover_text("The room goes on without you").clicked() {
                    app.send(UiCommand::SkipLink(q.item_id));
                }
            });
        });
    }
    if let (Some(room), Some(hold)) =
        (&snap.room, snap.playback.as_ref().and_then(|p| p.hold.as_ref()))
    {
        theme::callout(ui, YELLOW, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(waiting_line(hold, room, &snap.progress, snap.me));
                if snap.is_host && ui.button("Start without them").clicked() {
                    app.send(UiCommand::StartWithoutWaiting);
                }
            });
        });
    }
    if app.host.is_some() && app.config.host.media.is_empty() {
        theme::callout(ui, theme::ACCENT, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("Nothing is shared yet: add a folder in Settings → My media folders.");
                if ui.button("Open settings").clicked() {
                    app.ui.settings_open = true;
                }
            });
        });
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn version_text_names_the_app_and_the_protocol() {
        let t = super::version_text();
        assert!(t.starts_with(&format!("sppy {}", env!("CARGO_PKG_VERSION"))), "{t}");
        assert!(t.ends_with(&format!("protocol v{}", sppy_proto::PROTOCOL_VERSION)), "{t}");
    }
}
