//! UI panels.

pub mod connect;
pub mod library;
pub mod participants;
pub mod playlist;

use sppy_client_core::api::Snapshot;

use crate::app::{LogKind, SppyApp};

/// Last error (red) or last notice, one line.
pub fn status_line(app: &mut SppyApp, ui: &mut egui::Ui, _snap: &Snapshot) {
    ui.horizontal(|ui| {
        if let Some(err) = &app.ui.last_error {
            ui.colored_label(ui.visuals().error_fg_color, err);
            if ui.small_button("✕").clicked() {
                app.ui.last_error = None;
            }
        } else if let Some((_, text)) = app.ui.log.iter().rev().find(|(k, _)| *k == LogKind::Notice)
        {
            ui.weak(text);
        }
    });
}
