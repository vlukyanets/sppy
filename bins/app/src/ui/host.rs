//! Host-only room settings: control mode, what happens to latecomers, invite rotation.

use sppy_client_core::api::{Snapshot, UiCommand};
use sppy_proto::{ControlMode, LagPolicy, RoomState};

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
    for cmd in host_controls(ui, room) {
        app.send(cmd);
    }
}

/// The host's controls for `room`; returns the commands the clicks asked for.
fn host_controls(ui: &mut egui::Ui, room: &RoomState) -> Vec<UiCommand> {
    let mut sent = Vec::new();
    ui.horizontal_wrapped(|ui| {
        let mut mode = room.control_mode;
        egui::ComboBox::from_id_salt("control-mode")
            .selected_text(mode_text(mode))
            .show_ui(ui, |ui| {
                for m in [ControlMode::Everyone, ControlMode::HostOnly, ControlMode::Selected] {
                    ui.selectable_value(&mut mode, m, mode_text(m));
                }
            })
            .response
            .on_hover_text("Who may play, pause, seek and change the playlist");
        if mode != room.control_mode {
            sent.push(UiCommand::SetControlMode(mode));
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
            sent.push(UiCommand::SetLagPolicy(policy));
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
            sent.push(UiCommand::SetReadyCheck(check));
        }
        if ui.button("Rotate invite").on_hover_text("Old invite links stop working").clicked() {
            sent.push(UiCommand::RotateInvite);
        }
    });
    if room.control_mode == ControlMode::Selected {
        ui.weak("Grant control with ⋯ next to a participant.");
    }
    sent
}

#[cfg(test)]
mod tests {
    use egui::accesskit::Role;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use sppy_proto::*;

    use super::*;
    use crate::ui::theme;

    /// The content width of the participants panel at its smallest (240 px minus margins).
    const NARROW: f32 = 212.0;
    /// The margin the harness puts around the UI on every side.
    const HARNESS_MARGIN: f32 = 8.0;

    fn room() -> RoomState {
        RoomState {
            room_id: RoomId::main(),
            participants: vec![],
            control_mode: ControlMode::Everyone,
            lag_policy: LagPolicy::CatchUpAlone,
            ready_check: false,
            repeat: Repeat::Off,
            playlist: vec![],
            settings: RoomSettings { stall_hold_secs: 10, resume_buffer_secs: 10 },
        }
    }

    #[test]
    fn host_controls_fit_a_narrow_panel() {
        let room = room();
        let mut h =
            Harness::builder().with_size([NARROW + 2.0 * HARNESS_MARGIN, 160.0]).build_ui_state(
                move |ui, sent: &mut Vec<UiCommand>| {
                    theme::apply(ui.ctx());
                    sent.extend(host_controls(ui, &room));
                },
                Vec::new(),
            );
        h.run();
        let mut controls: Vec<_> = h.get_all_by_role(Role::ComboBox).map(|n| n.rect()).collect();
        assert_eq!(controls.len(), 2, "control mode and lag policy");
        controls.push(h.get_by_role_and_label(Role::CheckBox, "Start on Ready").rect());
        controls.push(h.get_by_role_and_label(Role::Button, "Rotate invite").rect());
        for rect in controls {
            assert!(
                rect.right() <= HARNESS_MARGIN + NARROW,
                "{rect:?} sticks out of a {NARROW} px panel"
            );
        }
        h.snapshot("host_controls_narrow");
    }
}
