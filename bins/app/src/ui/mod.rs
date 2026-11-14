//! UI panels.

pub mod chat;
pub mod connect;
pub mod host;
pub mod library;
pub mod participants;
pub mod playlist;
pub mod settings;

use egui::Color32;
use sppy_client_core::api::{Snapshot, UiCommand};
use sppy_client_core::notice::waiting_line;

use crate::app::SppyApp;

pub const GREEN: Color32 = Color32::from_rgb(90, 190, 110);
pub const YELLOW: Color32 = Color32::from_rgb(230, 180, 60);
pub const RED: Color32 = Color32::from_rgb(225, 85, 80);

/// Last error (red, dismissable) or last notice, one line.
pub fn status_line(app: &mut SppyApp, ui: &mut egui::Ui, _snap: &Snapshot) {
    ui.horizontal(|ui| {
        if let Some(err) = &app.ui.last_error {
            ui.colored_label(ui.visuals().error_fg_color, err);
            if ui.small_button("✕").clicked() {
                app.ui.last_error = None;
            }
        } else if let Some(e) = app.ui.log.last_notice() {
            ui.weak(e.display());
        }
    });
}

/// Things that need attention: the player could not start, the room is waiting,
/// the host has no media folders.
pub fn banners(app: &mut SppyApp, ui: &mut egui::Ui, snap: &Snapshot) {
    if let Some(problem) = app.ui.player_problem.clone() {
        egui::Frame::group(ui.style()).show(ui, |ui| {
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
    if let (Some(room), Some(hold)) =
        (&snap.room, snap.playback.as_ref().and_then(|p| p.hold.as_ref()))
    {
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(YELLOW, waiting_line(hold, room, &snap.progress, snap.me));
            if snap.is_host && ui.small_button("Start without them").clicked() {
                app.send(UiCommand::StartWithoutWaiting);
            }
        });
    }
    if app.host.is_some() && app.config.host.media.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(
                YELLOW,
                "The library is empty: add a media folder in Settings → Hosting.",
            );
            if ui.small_button("Open settings").clicked() {
                app.ui.settings_open = true;
            }
        });
    }
}
