//! Chat and the event log in one list (events can be hidden), plus the input line.

use sppy_client_core::api::{Snapshot, UiCommand};

use super::theme;
use crate::app::{LogKind, SppyApp};

pub fn show(app: &mut SppyApp, ui: &mut egui::Ui, snap: &Snapshot) {
    // Seed the log with the history the server sent on join (once per session).
    if snap.me.is_some() && app.ui.chat_seeded_for != snap.me {
        app.ui.chat_seeded_for = snap.me;
        for c in snap.chat_history.iter().rev() {
            app.ui.log.push_history_front(format!("{}: {}", c.nickname, c.text));
        }
    }
    ui.horizontal(|ui| {
        theme::section(ui, "Chat");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.checkbox(&mut app.ui.show_events, egui::RichText::new("events").small());
        });
    });
    let input_height = 40.0;
    egui::Frame::new()
        .fill(ui.visuals().panel_fill)
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .max_height((ui.available_height() - input_height).max(80.0))
                .stick_to_bottom(true)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    for e in app.ui.log.iter() {
                        match e.kind {
                            LogKind::Chat => {
                                let text = e.display();
                                ui.horizontal_wrapped(|ui| {
                                    ui.spacing_mut().item_spacing.x = 0.0;
                                    match text.split_once(": ") {
                                        Some((nick, msg)) => {
                                            ui.label(
                                                egui::RichText::new(format!("{nick}  "))
                                                    .strong()
                                                    .color(theme::ACCENT),
                                            );
                                            ui.label(msg);
                                        }
                                        None => {
                                            ui.label(text);
                                        }
                                    }
                                });
                            }
                            LogKind::Notice if app.ui.show_events => {
                                ui.label(
                                    egui::RichText::new(e.display()).small().color(theme::MUTED),
                                );
                            }
                            LogKind::Error => {
                                ui.colored_label(ui.visuals().error_fg_color, e.display());
                            }
                            LogKind::Notice => {}
                        }
                    }
                });
        });
    ui.add_space(6.0);
    let r = ui.add(
        egui::TextEdit::singleline(&mut app.ui.chat)
            .hint_text("Message…")
            .margin(egui::Margin::symmetric(10, 6))
            .desired_width(f32::INFINITY),
    );
    if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        let text = app.ui.chat.trim().to_owned();
        if !text.is_empty() {
            app.send(UiCommand::Chat(text));
        }
        app.ui.chat.clear();
        r.request_focus();
    }
}
