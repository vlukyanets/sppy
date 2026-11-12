//! Participants: badges, connection, status and delivery progress.

use sppy_client_core::api::{Snapshot, UiCommand};
use sppy_proto::{ControlMode, Delivery, MediaStatus};

use crate::app::SppyApp;

pub fn status_text(s: &MediaStatus) -> String {
    match s {
        MediaStatus::Loading => "loading".into(),
        MediaStatus::Ready { .. } => "ready".into(),
        MediaStatus::Stalled => "buffering".into(),
        MediaStatus::Failed { message } => format!("failed: {message}"),
        MediaStatus::PlayerClosed => "player closed".into(),
    }
}

pub fn show(app: &mut SppyApp, ui: &mut egui::Ui, snap: &Snapshot) {
    let Some(room) = &snap.room else { return };
    for p in &room.participants {
        ui.horizontal(|ui| {
            let dot = if p.connected { "●" } else { "○" };
            ui.label(dot).on_hover_text(if p.connected { "connected" } else { "disconnected" });
            let name = if Some(p.id) == snap.me {
                format!("{} (you)", p.nickname)
            } else {
                p.nickname.clone()
            };
            ui.strong(name);
            if p.is_host {
                ui.small("host");
            } else if p.can_control {
                ui.small("ctrl");
            }
            ui.weak(status_text(&p.status));
            match snap.progress.get(&p.id).map(|e| &e.delivery) {
                Some(Delivery::Download { bytes, total }) if *total > 0 => {
                    let f = *bytes as f32 / *total as f32;
                    ui.add(egui::ProgressBar::new(f).desired_width(90.0).show_percentage());
                }
                Some(Delivery::Stream) => {
                    ui.weak("stream");
                }
                Some(Delivery::Local) => {
                    ui.weak("local");
                }
                _ => {}
            }
            if snap.is_host && !p.is_host {
                if room.control_mode == ControlMode::Selected {
                    let mut allowed = p.can_control;
                    if ui.checkbox(&mut allowed, "ctrl").changed() {
                        app.send(UiCommand::SetCanControl(p.id, allowed));
                    }
                }
                if ui.small_button("Kick").clicked() {
                    app.ui.confirm_kick = Some((p.id, p.nickname.clone()));
                }
            }
        });
    }
}

/// Confirmation before removing someone from the room.
pub fn kick_dialog(app: &mut SppyApp, ctx: &egui::Context) {
    let Some((pid, name)) = app.ui.confirm_kick.clone() else { return };
    egui::Window::new("Remove participant").collapsible(false).resizable(false).show(ctx, |ui| {
        ui.label(format!("Remove {name} from the room?"));
        ui.horizontal(|ui| {
            if ui.button("Remove").clicked() {
                app.send(UiCommand::Kick(pid));
                app.ui.confirm_kick = None;
            }
            if ui.button("Cancel").clicked() {
                app.ui.confirm_kick = None;
            }
        });
    });
}
