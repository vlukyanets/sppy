//! Participants: colored status, delivery, sync indicator; host actions in a menu.

use sppy_client_core::api::{Snapshot, UiCommand};
use sppy_proto::{ControlMode, Delivery, MediaStatus};

use super::{GREEN, RED, YELLOW, theme};
use crate::app::SppyApp;

/// A word for the status pill, its colour, and the details behind it (shown on hover).
pub fn status_text(s: &MediaStatus) -> (&'static str, egui::Color32, Option<&str>) {
    let gray = egui::Color32::GRAY;
    match s {
        MediaStatus::Loading => ("loading", gray, None),
        MediaStatus::Ready { .. } => ("ready", GREEN, None),
        MediaStatus::Stalled => ("buffering", YELLOW, None),
        MediaStatus::Failed { message } => ("failed", RED, Some(message.as_str())),
        MediaStatus::PlayerClosed => ("player closed", RED, None),
    }
}

/// `±20 ms`, colored by how far the participant is from the room: green inside the dead
/// band, where sppy corrects nothing.
pub fn sync_text(drift_ms: i64, dead_band_ms: u64) -> (String, egui::Color32) {
    let abs = drift_ms.unsigned_abs();
    let color = if abs <= dead_band_ms {
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
        let id = ui.scope_id().with(("participant", p.id));
        theme::row(ui, id, false, |ui, _| {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.horizontal(|ui| {
                    let (dot, color) =
                        if p.connected { ("●", GREEN) } else { ("○", theme::MUTED) };
                    ui.colored_label(color, dot).on_hover_text(if p.connected {
                        "connected"
                    } else {
                        "disconnected"
                    });
                    let mut name = egui::RichText::new(&p.nickname).strong();
                    if !p.connected {
                        name = name.color(theme::MUTED);
                    }
                    ui.add(egui::Label::new(name).truncate());
                    if Some(p.id) == snap.me {
                        ui.label(egui::RichText::new("you").small().color(theme::MUTED));
                    }
                    if p.is_host {
                        theme::pill(ui, "host", theme::ACCENT);
                    } else if p.can_control && room.control_mode == ControlMode::Selected {
                        theme::pill(ui, "control", theme::ACCENT);
                    }
                    if snap.is_host && !p.is_host {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
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
                        });
                    }
                });
                // Second line: what their player is doing.
                ui.horizontal_wrapped(|ui| {
                    ui.add_space(18.0);
                    let (status, color, details) = status_text(&p.status);
                    let pill = theme::pill(ui, status, color);
                    if let Some(details) = details {
                        pill.on_hover_text(details);
                    }
                    let entry = snap.progress.get(&p.id);
                    match entry.map(|e| &e.delivery) {
                        Some(Delivery::Download { bytes, total })
                            if *total > 0 && bytes < total =>
                        {
                            ui.add(
                                egui::ProgressBar::new(*bytes as f32 / *total as f32)
                                    .desired_width(60.0)
                                    .desired_height(8.0)
                                    .fill(theme::ACCENT),
                            )
                            .on_hover_text("Downloading");
                        }
                        Some(Delivery::Stream) => {
                            ui.label(egui::RichText::new("stream").small().color(theme::MUTED));
                        }
                        Some(Delivery::Local | Delivery::Download { .. }) => {
                            ui.label(egui::RichText::new("local").small().color(theme::MUTED));
                        }
                        None => {}
                    }
                    if playing && let Some(d) = entry.and_then(|e| e.drift_ms) {
                        let (text, color) = sync_text(d, app.config.advanced.dead_band_ms);
                        ui.label(egui::RichText::new(text).small().monospace().color(color))
                            .on_hover_text("Distance from the room position");
                    }
                });
            });
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

    /// A long failure message would push the rest of the line around: it goes into the
    /// hover text, the pill says only "failed".
    #[test]
    fn failure_details_stay_out_of_the_pill() {
        let failed = MediaStatus::Failed { message: "mpv could not open the media".into() };
        assert_eq!(status_text(&failed), ("failed", RED, Some("mpv could not open the media")));
        assert_eq!(status_text(&MediaStatus::Loading).2, None);
    }

    #[test]
    fn sync_indicator_colors_and_format() {
        assert_eq!(sync_text(-20, 100), ("-20 ms".into(), GREEN));
        // Inside the dead band nothing gets corrected, so it counts as in sync.
        assert_eq!(sync_text(-90, 100), ("-90 ms".into(), GREEN));
        assert_eq!(sync_text(120, 100), ("+120 ms".into(), YELLOW));
        assert_eq!(sync_text(-2_500, 100), ("-2.5 s".into(), RED));
    }
}
