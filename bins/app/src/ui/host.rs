//! Host-only room settings: control mode, what happens to latecomers, invite rotation.

use sppy_client_core::api::{Snapshot, UiCommand};
use sppy_proto::{ControlMode, LagPolicy};

use crate::app::SppyApp;

pub fn mode_text(m: ControlMode) -> &'static str {
    match m {
        ControlMode::Everyone => "Everyone can control",
        ControlMode::HostOnly => "Only the host",
        ControlMode::Selected => "Selected participants",
    }
}

pub fn lag_text(p: LagPolicy) -> &'static str {
    match p {
        LagPolicy::CatchUpAlone => "Latecomers catch up alone",
        LagPolicy::FollowSlowest => "Wait for latecomers",
    }
}

fn lag_hint(p: LagPolicy) -> &'static str {
    match p {
        LagPolicy::CatchUpAlone => {
            "The room plays on; whoever falls behind catches up by themselves."
        }
        LagPolicy::FollowSlowest => {
            "When someone falls more than a second behind, the room goes back to them; the \
             others slow down or jump back."
        }
    }
}

pub fn show(app: &mut SppyApp, ui: &mut egui::Ui, snap: &Snapshot) {
    let Some(room) = &snap.room else { return };
    ui.horizontal(|ui| {
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
        let mut policy = room.lag_policy;
        egui::ComboBox::from_id_salt("lag-policy")
            .selected_text(lag_text(policy))
            .show_ui(ui, |ui| {
                for p in [LagPolicy::CatchUpAlone, LagPolicy::FollowSlowest] {
                    ui.selectable_value(&mut policy, p, lag_text(p)).on_hover_text(lag_hint(p));
                }
            })
            .response
            .on_hover_text(lag_hint(room.lag_policy));
        if policy != room.lag_policy {
            app.send(UiCommand::SetLagPolicy(policy));
        }
        let mut check = room.ready_check;
        if ui
            .checkbox(&mut check, "Start on Ready")
            .on_hover_text(
                "A new item starts once everyone has loaded it and pressed Ready, not as soon \
                 as it has loaded.",
            )
            .changed()
        {
            app.send(UiCommand::SetReadyCheck(check));
        }
        if ui.button("Rotate invite").on_hover_text("Old invite links stop working").clicked() {
            app.send(UiCommand::RotateInvite);
        }
    });
    if room.control_mode == ControlMode::Selected {
        ui.weak("Grant control with ⋯ next to a participant.");
    }
}
