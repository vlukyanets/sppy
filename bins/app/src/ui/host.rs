//! Host-only room settings: control mode, what happens to latecomers, invite rotation.

use sppy_client_core::api::{Snapshot, UiCommand};
use sppy_proto::{ControlMode, LagPolicy, RoomState};

use crate::app::SppyApp;
use crate::ui::theme;

pub fn mode_text(m: ControlMode) -> &'static str {
    match m {
        ControlMode::Everyone => "Everyone",
        ControlMode::HostOnly => "Only the host",
        ControlMode::Selected => "Selected participants",
    }
}

pub fn lag_text(p: LagPolicy) -> &'static str {
    match p {
        LagPolicy::CatchUpAlone => "Catch up alone",
        LagPolicy::FollowSlowest => "Room waits",
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

/// A label in the left column; never wrapped, the right column gets what is left.
fn row_label(ui: &mut egui::Ui, text: &str) {
    ui.add(egui::Label::new(egui::RichText::new(text).color(theme::MUTED)).extend());
}

/// The host's controls for `room`, as a small labelled form; returns the commands the clicks
/// asked for.
fn host_controls(ui: &mut egui::Ui, room: &RoomState) -> Vec<UiCommand> {
    let mut sent = Vec::new();
    theme::section(ui, "Room");
    egui::Frame::new()
        .fill(ui.visuals().faint_bg_color)
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::Grid::new("host-controls").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                let mut mode = room.control_mode;
                row_label(ui, "Control");
                egui::ComboBox::from_id_salt("control-mode")
                    .width(ui.available_width())
                    .truncate()
                    .selected_text(mode_text(mode))
                    .show_ui(ui, |ui| {
                        for m in
                            [ControlMode::Everyone, ControlMode::HostOnly, ControlMode::Selected]
                        {
                            ui.selectable_value(&mut mode, m, mode_text(m));
                        }
                    })
                    .response
                    .on_hover_text("Who may play, pause, seek and change the playlist");
                if mode != room.control_mode {
                    sent.push(UiCommand::SetControlMode(mode));
                }
                ui.end_row();

                let mut policy = room.lag_policy;
                row_label(ui, "Latecomers");
                egui::ComboBox::from_id_salt("lag-policy")
                    .width(ui.available_width())
                    .truncate()
                    .selected_text(lag_text(policy))
                    .show_ui(ui, |ui| {
                        for p in [LagPolicy::CatchUpAlone, LagPolicy::FollowSlowest] {
                            ui.selectable_value(&mut policy, p, lag_text(p))
                                .on_hover_text(lag_hint(p));
                        }
                    })
                    .response
                    .on_hover_text(lag_hint(room.lag_policy));
                if policy != room.lag_policy {
                    sent.push(UiCommand::SetLagPolicy(policy));
                }
                ui.end_row();

                let mut check = room.ready_check;
                row_label(ui, "Start");
                if theme::tick(ui, &mut check, "on Ready")
                    .on_hover_text(
                        "A new item starts once everyone has loaded it and ticked I'm ready, \
                         not as soon as it has loaded.",
                    )
                    .changed()
                {
                    sent.push(UiCommand::SetReadyCheck(check));
                }
                ui.end_row();
            });
            ui.add_space(4.0);
            if ui.button("Rotate invite").on_hover_text("Old invite links stop working").clicked() {
                sent.push(UiCommand::RotateInvite);
            }
            if room.control_mode == ControlMode::Selected {
                theme::muted(ui, "Grant control with ⋯ next to a participant.");
            }
        });
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
    /// The content width of the participants panel at its default size (300 px).
    const DEFAULT: f32 = 272.0;
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

    /// The controls in a panel whose content is `width` wide.
    fn controls(width: f32) -> Harness<'static, Vec<UiCommand>> {
        let room = room();
        let mut h =
            Harness::builder().with_size([width + 2.0 * HARNESS_MARGIN, 200.0]).build_ui_state(
                move |ui, sent: &mut Vec<UiCommand>| {
                    theme::apply(ui.ctx());
                    sent.extend(host_controls(ui, &room));
                },
                Vec::new(),
            );
        h.run();
        h
    }

    /// Both lists start at the same x (a column, not a jumble), and nothing sticks out of a
    /// panel `width` wide.
    fn assert_tidy(width: f32) {
        let h = controls(width);
        let combos: Vec<_> = h.get_all_by_role(Role::ComboBox).map(|n| n.rect()).collect();
        assert_eq!(combos.len(), 2, "control mode and lag policy");
        assert_eq!(combos[0].left(), combos[1].left(), "the lists form a column");
        let mut all = combos;
        all.push(h.get_by_role_and_label(Role::CheckBox, "on Ready").rect());
        all.push(h.get_by_role_and_label(Role::Button, "Rotate invite").rect());
        for rect in all {
            assert!(
                rect.right() <= HARNESS_MARGIN + width,
                "{rect:?} sticks out of a {width} px panel"
            );
        }
    }

    #[test]
    fn host_controls_fit_a_narrow_panel() {
        assert_tidy(NARROW);
    }

    #[test]
    fn host_controls_line_up_at_the_default_width() {
        assert_tidy(DEFAULT);
    }
}
