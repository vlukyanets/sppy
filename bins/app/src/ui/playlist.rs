//! What is playing (title and transport) and the playlist.

use sppy_client_core::api::{Snapshot, UiCommand};
use sppy_client_core::notice::fmt_time;
use sppy_proto::{ItemId, Repeat, RoomState, Source};

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

fn repeat_text(r: Repeat) -> &'static str {
    match r {
        Repeat::Off => "No repeat",
        Repeat::All => "Repeat all",
        Repeat::One => "Repeat one",
    }
}

/// Repeat and Shuffle above the list (for those who may edit the playlist); what to send.
fn playlist_controls(ui: &mut egui::Ui, room: &RoomState) -> Option<UiCommand> {
    let mut out = None;
    ui.horizontal(|ui| {
        let mut repeat = room.repeat;
        egui::ComboBox::from_id_salt("repeat")
            .selected_text(repeat_text(repeat))
            .show_ui(ui, |ui| {
                for r in [Repeat::Off, Repeat::All, Repeat::One] {
                    ui.selectable_value(&mut repeat, r, repeat_text(r));
                }
            })
            .response
            .on_hover_text("What comes after an item played to its end");
        if repeat != room.repeat {
            out = Some(UiCommand::SetRepeat(repeat));
        }
        if ui
            .add_enabled(room.playlist.len() > 1, egui::Button::new("Shuffle"))
            .on_hover_text("Put the items after the current one in a random order")
            .clicked()
        {
            out = Some(UiCommand::PlaylistShuffle);
        }
    });
    out
}

pub fn show(app: &mut SppyApp, ui: &mut egui::Ui, snap: &Snapshot) {
    let (Some(room), Some(pb)) = (&snap.room, &snap.playback) else { return };
    let can = snap.can_control();
    let ids: Vec<ItemId> = room.playlist.iter().map(|i| i.item_id).collect();

    if can
        && !room.playlist.is_empty()
        && let Some(cmd) = playlist_controls(ui, room)
    {
        app.send(cmd);
    }
    for (idx, item) in room.playlist.iter().enumerate() {
        let is_current = Some(item.item_id) == pb.item_id;
        let id = ui.scope_id().with(("playlist-row", item.item_id));
        theme::row(ui, id, is_current, |ui, hovered| {
            ui.label(
                egui::RichText::new(format!("{:>2}", idx + 1)).monospace().color(theme::MUTED),
            );
            let mut text = egui::RichText::new(&item.title);
            if is_current {
                text = text.strong().color(ui.visuals().strong_text_color());
            } else if item.watched {
                text = text.color(theme::MUTED);
            }
            if item.watched {
                ui.label(egui::RichText::new("✓").color(theme::MUTED)).on_hover_text("Watched");
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
                        let hint =
                            if item.watched { "Mark as not watched" } else { "Mark as watched" };
                        if theme::icon(ui, if item.watched { "○" } else { "✓" }, hint) {
                            app.send(UiCommand::SetWatched(item.item_id, !item.watched));
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

#[cfg(test)]
mod tests {
    use egui::accesskit::Role;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use sppy_proto::*;

    use super::*;

    fn room(items: u64, repeat: Repeat) -> RoomState {
        let item = |id| Item {
            item_id: ItemId(id),
            source: Source::Url { url: format!("https://x/{id}") },
            title: format!("Episode {id}"),
            duration_ms: None,
            fingerprint: None,
            subtitles: vec![],
            added_by: ParticipantId(1),
            watched: false,
        };
        RoomState {
            room_id: RoomId::main(),
            participants: vec![],
            control_mode: ControlMode::Everyone,
            lag_policy: LagPolicy::default(),
            ready_check: false,
            repeat,
            playlist: (1..=items).map(item).collect(),
            settings: RoomSettings { stall_hold_secs: 10, resume_buffer_secs: 10 },
        }
    }

    fn controls(room: RoomState) -> Harness<'static, Vec<UiCommand>> {
        let mut h = Harness::builder().with_size([360.0, 160.0]).build_ui_state(
            move |ui, sent: &mut Vec<UiCommand>| {
                theme::apply(ui.ctx());
                sent.extend(playlist_controls(ui, &room));
            },
            Vec::new(),
        );
        h.run();
        h
    }

    #[test]
    fn shuffle_is_sent_and_needs_two_items() {
        let mut h = controls(room(3, Repeat::Off));
        h.get_by_role_and_label(Role::Button, "Shuffle").click();
        h.run();
        assert!(matches!(h.state()[..], [UiCommand::PlaylistShuffle]));
        let mut h = controls(room(1, Repeat::Off));
        h.get_by_role_and_label(Role::Button, "Shuffle").click();
        h.run();
        assert!(h.state().is_empty(), "nothing to shuffle with one item");
    }

    #[test]
    fn repeat_is_picked_from_the_list() {
        let mut h = controls(room(3, Repeat::Off));
        let combo = h.get_by_role(Role::ComboBox);
        assert_eq!(combo.value().as_deref(), Some("No repeat"));
        combo.click();
        h.run();
        h.snapshot("playlist_controls_repeat_menu");
        h.get_by_label("Repeat one").click();
        h.run();
        assert!(matches!(h.state()[..], [UiCommand::SetRepeat(Repeat::One)]), "{:?}", h.state());
    }
}
