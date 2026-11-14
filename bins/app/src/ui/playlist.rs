//! Transport controls and the playlist.

use sppy_client_core::api::{Snapshot, UiCommand};
use sppy_client_core::notice::fmt_time;
use sppy_proto::{ItemId, Source};

use crate::app::SppyApp;

pub fn show(app: &mut SppyApp, ui: &mut egui::Ui, snap: &Snapshot) {
    let (Some(room), Some(pb)) = (&snap.room, &snap.playback) else { return };
    let can = snap.can_control();
    let ids: Vec<ItemId> = room.playlist.iter().map(|i| i.item_id).collect();
    let pos_in_list = pb.item_id.and_then(|id| ids.iter().position(|i| *i == id));
    let current = pos_in_list.map(|i| &room.playlist[i]);

    // Transport.
    ui.horizontal(|ui| {
        ui.add_enabled_ui(can, |ui| {
            let prev = pos_in_list.and_then(|i| i.checked_sub(1)).map(|i| ids[i]);
            if ui
                .add_enabled(prev.is_some(), egui::Button::new("⏮"))
                .on_hover_text("Previous")
                .clicked()
                && let Some(id) = prev
            {
                app.send(UiCommand::PlaylistSelect(id));
            }
            let label = if pb.paused { "▶ Play" } else { "⏸ Pause" };
            if ui.add_enabled(current.is_some(), egui::Button::new(label)).clicked() {
                app.send(if pb.paused { UiCommand::Play } else { UiCommand::Pause });
            }
            let next = pos_in_list.and_then(|i| ids.get(i + 1)).copied();
            if ui
                .add_enabled(next.is_some(), egui::Button::new("⏭"))
                .on_hover_text("Next")
                .clicked()
                && let Some(id) = next
            {
                app.send(UiCommand::PlaylistSelect(id));
            }
            if ui
                .add_enabled(current.is_some(), egui::Button::new("⟲"))
                .on_hover_text("From the start")
                .clicked()
            {
                app.send(UiCommand::Seek(0));
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
    ui.add_space(4.0);

    // Items.
    for (idx, item) in room.playlist.iter().enumerate() {
        let is_current = Some(item.item_id) == pb.item_id;
        let played = pos_in_list.is_some_and(|p| idx < p);
        ui.horizontal(|ui| {
            ui.label(if is_current { "▶" } else { "  " });
            let mut text = egui::RichText::new(&item.title);
            if is_current {
                text = text.strong();
            } else if played {
                text = text.weak();
            }
            let r = ui.add(egui::Label::new(text).truncate().sense(egui::Sense::click()));
            // Double-click plays another item; on the current one it would restart it for everyone.
            if can && !is_current && r.on_hover_text("Double-click to play").double_clicked() {
                app.send(UiCommand::PlaylistSelect(item.item_id));
            }
            if let Some(d) = item.duration_ms {
                ui.weak(fmt_time(d));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if can {
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
                }
                if matches!(item.source, Source::File { .. })
                    && ui
                        .small_button("My file…")
                        .on_hover_text("Play my own copy of this file")
                        .clicked()
                    && let Some(path) = rfd::FileDialog::new().pick_file()
                {
                    app.send(UiCommand::ChooseLocalFile { item_id: item.item_id, path });
                }
            });
        });
    }
    if room.playlist.is_empty() {
        ui.weak("The playlist is empty. Add files in the Library tab or a link below.");
    }

    if can {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut app.ui.url)
                    .hint_text("Link (YouTube, …), opened by mpv / yt-dlp"),
            );
            let url = app.ui.url.trim().to_owned();
            if ui.add_enabled(!url.is_empty(), egui::Button::new("Add link")).clicked() {
                let url = sppy_client_core::media::normalize_url(&url);
                app.send(UiCommand::PlaylistAdd(vec![Source::Url { url }]));
                app.ui.url.clear();
            }
        });
    }
}
