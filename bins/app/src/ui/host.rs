//! Host-only controls: control mode, invite rotation.

use sppy_client_core::api::{Snapshot, UiCommand};
use sppy_proto::ControlMode;

use crate::app::SppyApp;

pub fn mode_text(m: ControlMode) -> &'static str {
    match m {
        ControlMode::Everyone => "Everyone can control",
        ControlMode::HostOnly => "Only the host",
        ControlMode::Selected => "Selected participants",
    }
}

pub fn show(app: &mut SppyApp, ui: &mut egui::Ui, snap: &Snapshot) {
    let Some(room) = &snap.room else { return };
    ui.horizontal(|ui| {
        ui.label("Control:");
        let mut mode = room.control_mode;
        egui::ComboBox::from_id_salt("control-mode").selected_text(mode_text(mode)).show_ui(
            ui,
            |ui| {
                for m in [ControlMode::Everyone, ControlMode::HostOnly, ControlMode::Selected] {
                    ui.selectable_value(&mut mode, m, mode_text(m));
                }
            },
        );
        if mode != room.control_mode {
            app.send(UiCommand::SetControlMode(mode));
        }
    });
    ui.horizontal(|ui| {
        if ui.button("Rotate invite").on_hover_text("Old invite links stop working").clicked() {
            app.send(UiCommand::RotateInvite);
        }
        if room.control_mode == ControlMode::Selected {
            ui.weak("Grant control with the checkboxes in Participants.");
        }
    });
}
