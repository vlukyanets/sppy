//! Transport controls, hold banner and the playlist.

use sppy_client_core::api::{Snapshot, UiCommand};
use sppy_client_core::notice::{fmt_time, nickname};
use sppy_proto::{ItemId, Source};

use crate::app::SppyApp;

pub fn show(app: &mut SppyApp, ui: &mut egui::Ui, snap: &Snapshot) {
    let (Some(room), Some(pb)) = (&snap.room, &snap.playback) else { return };
    let can = snap.can_control();
    let current = pb.item_id.and_then(|id| room.playlist.iter().find(|i| i.item_id == id));

    // Transport.
    ui.horizontal(|ui| {
        ui.add_enabled_ui(can && current.is_some(), |ui| {
            let label = if pb.paused { "▶ Play" } else { "⏸ Pause" };
            if ui.button(label).clicked() {
                app.send(if pb.paused { UiCommand::Play } else { UiCommand::Pause });
            }
        });
        let pos = app.handle.expected_position_ms().unwrap_or(pb.position_ms);
        match current.and_then(|i| i.duration_ms) {
            Some(d) => {
                let mut v = pos.min(d) as f64;
                let slider = egui::Slider::new(&mut v, 0.0..=d as f64).show_value(false);
                let r = ui.add_enabled(can, slider);
                if r.drag_stopped() || (r.changed() && !r.dragged()) {
                    app.send(UiCommand::Seek(v as u64));
                }
                ui.monospace(format!("{} / {}", fmt_time(pos), fmt_time(d)));
            }
            None => {
                ui.monospace(fmt_time(pos));
            }
        }
        if pb.speed != 1.0 {
            ui.weak(format!("{}×", pb.speed));
        }
    });
    if let Some(hold) = &pb.hold {
        let names: Vec<String> = hold.waiting_for.iter().map(|p| nickname(room, *p)).collect();
        ui.colored_label(ui.visuals().warn_fg_color, format!("Waiting for: {}", names.join(", ")));
    }

    // Items.
    let ids: Vec<ItemId> = room.playlist.iter().map(|i| i.item_id).collect();
    for (idx, item) in room.playlist.iter().enumerate() {
        ui.horizontal(|ui| {
            let is_current = Some(item.item_id) == pb.item_id;
            ui.label(if is_current { "▶" } else { " " });
            let title = egui::Label::new(if is_current {
                egui::RichText::new(&item.title).strong()
            } else {
                egui::RichText::new(&item.title)
            })
            .truncate()
            .sense(egui::Sense::click());
            let r = ui.add(title).on_hover_text("Double-click to play");
            if can && r.double_clicked() {
                app.send(UiCommand::PlaylistSelect(item.item_id));
            }
            if let Some(d) = item.duration_ms {
                ui.weak(fmt_time(d));
            }
            if matches!(item.source, Source::File { .. })
                && ui.small_button("📁").on_hover_text("Use my own copy of this file").clicked()
                && let Some(path) = rfd::FileDialog::new().pick_file()
            {
                app.send(UiCommand::ChooseLocalFile { item_id: item.item_id, path });
            }
            if can {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("✕").on_hover_text("Remove").clicked() {
                        app.send(UiCommand::PlaylistRemove(item.item_id));
                    }
                    if idx + 1 < ids.len() && ui.small_button("▼").clicked() {
                        app.send(UiCommand::PlaylistMove {
                            item_id: item.item_id,
                            after: Some(ids[idx + 1]),
                        });
                    }
                    if idx > 0 && ui.small_button("▲").clicked() {
                        let after = if idx >= 2 { Some(ids[idx - 2]) } else { None };
                        app.send(UiCommand::PlaylistMove { item_id: item.item_id, after });
                    }
                });
            }
        });
    }
    if room.playlist.is_empty() {
        ui.weak("The playlist is empty. Add files from the library or a URL.");
    }

    // Add URL.
    if can {
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut app.ui.url)
                    .hint_text("https://… (opened by mpv / yt-dlp)"),
            );
            let url = app.ui.url.trim().to_owned();
            if ui.add_enabled(!url.is_empty(), egui::Button::new("Add URL")).clicked() {
                let url = sppy_client_core::media::normalize_url(&url);
                app.send(UiCommand::PlaylistAdd(vec![Source::Url { url }]));
                app.ui.url.clear();
            }
        });
    }
}
