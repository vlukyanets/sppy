//! What is playing (title and transport) and the playlist.

use sppy_client_core::api::{Snapshot, UiCommand};
use sppy_client_core::notice::fmt_time;
use sppy_proto::{ItemId, Source};

use super::theme;
use crate::app::SppyApp;

/// The current item's title and the transport: previous, play/pause, next, the position.
pub fn now_playing(app: &mut SppyApp, ui: &mut egui::Ui, snap: &Snapshot) {
    let (Some(room), Some(pb)) = (&snap.room, &snap.playback) else { return };
    let can = snap.can_control();
    let ids: Vec<ItemId> = room.playlist.iter().map(|i| i.item_id).collect();
    let pos_in_list = pb.item_id.and_then(|id| ids.iter().position(|i| *i == id));
    let current = pos_in_list.map(|i| &room.playlist[i]);

    egui::Frame::new()
        .fill(ui.visuals().faint_bg_color)
        .corner_radius(egui::CornerRadius::same(10))
        .inner_margin(egui::Margin::symmetric(16, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            match current {
                Some(item) => {
                    ui.label(
                        egui::RichText::new("NOW PLAYING").small().strong().color(theme::MUTED),
                    );
                    ui.add(
                        egui::Label::new(egui::RichText::new(&item.title).heading().strong())
                            .truncate(),
                    );
                }
                None => {
                    ui.label(
                        egui::RichText::new("Nothing is playing").heading().color(theme::MUTED),
                    );
                }
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.add_enabled_ui(can, |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    let prev = pos_in_list.and_then(|i| i.checked_sub(1)).map(|i| ids[i]);
                    let next = pos_in_list.and_then(|i| ids.get(i + 1)).copied();
                    let has = current.is_some();
                    let button = |ui: &mut egui::Ui, on: bool, icon, main: bool, hint: &str| {
                        let size = if main { 38.0 } else { 32.0 };
                        ui.add_enabled_ui(on, |ui| theme::transport(ui, icon, size, main))
                            .inner
                            .on_hover_text(hint)
                            .clicked()
                    };
                    if button(ui, prev.is_some(), theme::Transport::Previous, false, "Previous")
                        && let Some(id) = prev
                    {
                        app.send(UiCommand::PlaylistSelect(id));
                    }
                    let (icon, hint) = if pb.paused {
                        (theme::Transport::Play, "Play")
                    } else {
                        (theme::Transport::Pause, "Pause")
                    };
                    if button(ui, has, icon, true, hint) {
                        app.send(if pb.paused { UiCommand::Play } else { UiCommand::Pause });
                    }
                    if button(ui, next.is_some(), theme::Transport::Next, false, "Next")
                        && let Some(id) = next
                    {
                        app.send(UiCommand::PlaylistSelect(id));
                    }
                    if button(ui, has, theme::Transport::Restart, false, "From the start") {
                        app.send(UiCommand::Seek(0));
                    }
                });
                let pos = app.handle.expected_position_ms().unwrap_or(pb.position_ms);
                let duration = current.and_then(|i| i.duration_ms);
                let times = match duration {
                    Some(d) => format!("{} / {}", fmt_time(pos.min(d)), fmt_time(d)),
                    None => fmt_time(pos),
                };
                let speed =
                    if pb.speed != 1.0 { format!("  {}×", pb.speed) } else { String::new() };
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(format!("{times}{speed}"))
                            .monospace()
                            .color(theme::MUTED),
                    );
                    if let Some(d) = duration {
                        ui.spacing_mut().slider_width = ui.available_width().max(60.0);
                        let mut v = pos.min(d) as f64;
                        let slider = egui::Slider::new(&mut v, 0.0..=d as f64).show_value(false);
                        let r = ui.add_enabled(can, slider);
                        if r.drag_stopped() || (r.changed() && !r.dragged()) {
                            app.send(UiCommand::Seek(v as u64));
                        }
                    }
                });
            });
        });
}

pub fn show(app: &mut SppyApp, ui: &mut egui::Ui, snap: &Snapshot) {
    let (Some(room), Some(pb)) = (&snap.room, &snap.playback) else { return };
    let can = snap.can_control();
    let ids: Vec<ItemId> = room.playlist.iter().map(|i| i.item_id).collect();
    let pos_in_list = pb.item_id.and_then(|id| ids.iter().position(|i| *i == id));

    for (idx, item) in room.playlist.iter().enumerate() {
        let is_current = Some(item.item_id) == pb.item_id;
        let played = pos_in_list.is_some_and(|p| idx < p);
        let id = ui.scope_id().with(("playlist-row", item.item_id));
        theme::row(ui, id, is_current, |ui, hovered| {
            ui.label(
                egui::RichText::new(format!("{:>2}", idx + 1)).monospace().color(theme::MUTED),
            );
            let mut text = egui::RichText::new(&item.title);
            if is_current {
                text = text.strong().color(ui.visuals().strong_text_color());
            } else if played {
                text = text.color(theme::MUTED);
            }
            let r = ui.add(egui::Label::new(text).truncate().sense(egui::Sense::click()));
            // Double-click plays another item; on the current one it would restart it for everyone.
            if can && !is_current && r.on_hover_text("Double-click to play").double_clicked() {
                app.send(UiCommand::PlaylistSelect(item.item_id));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if hovered {
                    if can {
                        if theme::icon(ui, "✕", "Remove") {
                            app.send(UiCommand::PlaylistRemove(item.item_id));
                        }
                        if idx + 1 < ids.len() && theme::icon(ui, "▼", "Move down") {
                            app.send(UiCommand::PlaylistMove {
                                item_id: item.item_id,
                                after: Some(ids[idx + 1]),
                            });
                        }
                        if idx > 0 && theme::icon(ui, "▲", "Move up") {
                            let after = if idx >= 2 { Some(ids[idx - 2]) } else { None };
                            app.send(UiCommand::PlaylistMove { item_id: item.item_id, after });
                        }
                    }
                    if matches!(item.source, Source::File { .. })
                        && theme::icon(ui, "My file…", "Play my own copy of this file")
                        && let Some(path) = rfd::FileDialog::new().pick_file()
                    {
                        app.send(UiCommand::ChooseLocalFile { item_id: item.item_id, path });
                    }
                } else if let Some(d) = item.duration_ms {
                    ui.label(egui::RichText::new(fmt_time(d)).monospace().color(theme::MUTED));
                } else if matches!(item.source, Source::Url { .. }) {
                    theme::pill(ui, "link", theme::MUTED);
                }
            });
        });
    }
    if room.playlist.is_empty() {
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(
                "The playlist is empty. Add files in the Library tab or a link below.",
            )
            .color(theme::MUTED),
        );
    }

    if can {
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut app.ui.url)
                    .hint_text("Paste a link (YouTube, …)")
                    .desired_width((ui.available_width() - 110.0).max(160.0)),
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
