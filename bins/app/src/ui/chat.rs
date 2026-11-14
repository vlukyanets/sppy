//! Chat and the event log in one list (events can be hidden), plus the input line.

use sppy_client_core::api::{Snapshot, UiCommand};

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
        ui.heading("Chat");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.checkbox(&mut app.ui.show_events, "Show events");
        });
    });
    let input_height = 30.0;
    egui::ScrollArea::vertical()
        .max_height((ui.available_height() - input_height).max(80.0))
        .stick_to_bottom(true)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for e in app.ui.log.iter() {
                match e.kind {
                    LogKind::Chat => {
                        ui.label(e.display());
                    }
                    LogKind::Notice if app.ui.show_events => {
                        ui.label(egui::RichText::new(e.display()).italics().weak());
                    }
                    LogKind::Error => {
                        ui.colored_label(ui.visuals().error_fg_color, e.display());
                    }
                    LogKind::Notice => {}
                }
            }
        });
    ui.horizontal(|ui| {
        let r = ui.add(
            egui::TextEdit::singleline(&mut app.ui.chat)
                .hint_text("Message")
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
    });
}
