//! Participants: colored status, delivery, sync indicator; host actions in a menu.

use sppy_client_core::api::{Snapshot, UiCommand};
use sppy_proto::{ControlMode, Delivery, MediaStatus};

use super::{GREEN, RED, YELLOW};
use crate::app::SppyApp;

pub fn status_text(s: &MediaStatus) -> (String, egui::Color32) {
    let gray = egui::Color32::GRAY;
    match s {
        MediaStatus::Loading => ("loading".into(), gray),
        MediaStatus::Ready { .. } => ("ready".into(), GREEN),
        MediaStatus::Stalled => ("buffering".into(), YELLOW),
        MediaStatus::Failed { message } => (format!("failed: {message}"), RED),
        MediaStatus::PlayerClosed => ("player closed".into(), RED),
    }
}

/// `±20 ms`, colored by how far the participant is from the room.
pub fn sync_text(drift_ms: i64) -> (String, egui::Color32) {
    let abs = drift_ms.unsigned_abs();
    let color = if abs < 80 {
        GREEN
    } else if abs < 500 {
        YELLOW
    } else {
        RED
    };
    let text = if abs < 1000 {
        format!("{drift_ms:+} ms")
    } else {
        format!("{:+.1} s", drift_ms as f64 / 1000.0)
    };
    (text, color)
}

pub fn show(app: &mut SppyApp, ui: &mut egui::Ui, snap: &Snapshot) {
    let Some(room) = &snap.room else { return };
    let playing = snap.playback.as_ref().is_some_and(|p| p.is_playing());
    for p in &room.participants {
        ui.horizontal(|ui| {
            let (dot, color) =
                if p.connected { ("●", GREEN) } else { ("○", egui::Color32::GRAY) };
            ui.colored_label(color, dot).on_hover_text(if p.connected {
                "connected"
            } else {
                "disconnected"
            });
            let name = if Some(p.id) == snap.me {
                format!("{} (you)", p.nickname)
            } else {
                p.nickname.clone()
            };
            ui.strong(name);
            if p.is_host {
                ui.small("host");
            } else if p.can_control && room.control_mode == ControlMode::Selected {
                ui.small("ctrl");
            }
            let (status, color) = status_text(&p.status);
            ui.colored_label(color, status);
            let entry = snap.progress.get(&p.id);
            match entry.map(|e| &e.delivery) {
                Some(Delivery::Download { bytes, total }) if *total > 0 && bytes < total => {
                    ui.add(
                        egui::ProgressBar::new(*bytes as f32 / *total as f32)
                            .desired_width(70.0)
                            .show_percentage(),
                    );
                }
                Some(Delivery::Stream) => {
                    ui.weak("stream");
                }
                Some(Delivery::Local | Delivery::Download { .. }) => {
                    ui.weak("local");
                }
                None => {}
            }
            if playing && let Some(d) = entry.and_then(|e| e.drift_ms) {
                let (text, color) = sync_text(d);
                ui.colored_label(color, text).on_hover_text("Distance from the room position");
            }
            if snap.is_host && !p.is_host {
                ui.menu_button("⋯", |ui| {
                    if room.control_mode == ControlMode::Selected {
                        let mut allowed = p.can_control;
                        if ui.checkbox(&mut allowed, "May control playback").changed() {
                            app.send(UiCommand::SetCanControl(p.id, allowed));
                        }
                    }
                    if ui.button("Remove from the room…").clicked() {
                        app.ui.confirm_kick = Some((p.id, p.nickname.clone()));
                        ui.close();
                    }
                });
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_indicator_colors_and_format() {
        assert_eq!(sync_text(-20), ("-20 ms".into(), GREEN));
        assert_eq!(sync_text(120), ("+120 ms".into(), YELLOW));
        assert_eq!(sync_text(-2_500), ("-2.5 s".into(), RED));
    }
}
