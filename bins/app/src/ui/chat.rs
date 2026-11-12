//! Chat and notices in one log, plus the input line.

use sppy_client_core::api::{Snapshot, UiCommand};

use crate::app::{LogKind, SppyApp};

pub fn show(app: &mut SppyApp, ui: &mut egui::Ui, snap: &Snapshot) {
    // Seed the log with the history the server sent on join (once per session).
    if snap.me.is_some() && app.ui.chat_seeded_for != snap.me {
        app.ui.chat_seeded_for = snap.me;
        for c in snap.chat_history.iter().rev() {
            app.ui.log.push_front((LogKind::Chat, format!("{}: {}", c.nickname, c.text)));
        }
    }
    egui::ScrollArea::vertical().max_height(180.0).stick_to_bottom(true).show(ui, |ui| {
        for (kind, text) in &app.ui.log {
            match kind {
                LogKind::Chat => ui.label(text),
                LogKind::Notice => ui.label(egui::RichText::new(text).italics().weak()),
                LogKind::Error => ui.colored_label(ui.visuals().error_fg_color, text),
            };
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
