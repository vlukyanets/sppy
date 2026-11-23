//! UI panels.

pub mod chat;
pub mod connect;
pub mod host;
pub mod library;
pub mod numbers;
pub mod participants;
pub mod playlist;
pub mod settings;
pub mod theme;

use egui::Color32;
use sppy_client_core::api::{Snapshot, UiCommand};
use sppy_client_core::notice::waiting_line;

use crate::app::SppyApp;

pub const GREEN: Color32 = Color32::from_rgb(98, 200, 140);
pub const YELLOW: Color32 = Color32::from_rgb(232, 186, 82);
pub const RED: Color32 = Color32::from_rgb(232, 96, 96);

/// "sppy 0.2.0 · protocol v3": both sides of a room need the same protocol.
pub fn version_text() -> String {
    format!("sppy {} · protocol v{}", env!("CARGO_PKG_VERSION"), sppy_proto::PROTOCOL_VERSION)
}

/// Last error (red, dismissable) or last notice on the left, the version on the right.
pub fn status_line(app: &mut SppyApp, ui: &mut egui::Ui, _snap: &Snapshot) {
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(egui::RichText::new(version_text()).small().color(theme::MUTED));
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                if let Some(err) = &app.ui.last_error {
                    ui.colored_label(ui.visuals().error_fg_color, err);
                    if theme::icon(ui, "✕", "Dismiss") {
                        app.ui.last_error = None;
                    }
                } else if let Some(e) = app.ui.log.last_notice() {
                    ui.label(egui::RichText::new(e.display()).small().color(theme::MUTED));
                }
            });
        });
    });
}

/// Things that need attention: the player could not start, a link to an unknown site,
/// the room is waiting, the host has no media folders.
/// The "I'm ready" tick while a new item waits for everyone's Ready; the new value when
/// it was clicked.
fn ready_button(
    ui: &mut egui::Ui,
    room: &sppy_proto::RoomState,
    hold: &sppy_proto::Hold,
    me: Option<sppy_proto::ParticipantId>,
) -> Option<bool> {
    let me = room.participants.iter().find(|p| Some(p.id) == me)?;
    if !room.ready_check || hold.reason != sppy_proto::HoldReason::ItemStart {
        return None;
    }
    let mut ready = me.ready;
    let text = egui::RichText::new("I'm ready").strong();
    let changed = ui
        .scope(|ui| {
            // Bigger than the other ticks: this one is what the room is waiting for.
            ui.spacing_mut().icon_width = 20.0;
            ui.spacing_mut().icon_width_inner = 12.0;
            ui.checkbox(&mut ready, text)
                .on_hover_text("The item starts once everyone has ticked it")
                .changed()
        })
        .inner;
    changed.then_some(ready)
}

pub fn banners(app: &mut SppyApp, ui: &mut egui::Ui, snap: &Snapshot) {
    if let Some(problem) = app.ui.player_problem.clone() {
        theme::callout(ui, RED, |ui| {
            ui.colored_label(RED, problem);
            ui.horizontal(|ui| {
                if ui.button("Open settings").clicked() {
                    app.ui.settings_open = true;
                }
                if ui.button("Try again").clicked() {
                    app.restart_player();
                }
            });
        });
    }
    if let Some(link) = app.ui.invite_received.clone() {
        let host = sppy_common::link::parse_address(&link).map(|a| a.host).unwrap_or_default();
        theme::callout(ui, theme::ACCENT, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(format!(
                    "An invite link to {host} is waiting in the join form: leave this room to use it."
                ));
                if ui.button("Dismiss").clicked() {
                    app.ui.invite_received = None;
                }
            });
        });
    }
    if let Some(q) = &snap.link_question {
        theme::callout(ui, YELLOW, |ui| {
            ui.label(format!(
                "{} added a link to {}. Your player will connect to that site.",
                q.added_by, q.site
            ));
            ui.horizontal_wrapped(|ui| {
                if ui.add(theme::primary("Play")).on_hover_text("Open this link once").clicked() {
                    app.send(UiCommand::AllowLink(q.item_id));
                }
                if ui
                    .button(format!("Always allow {}", q.site))
                    .on_hover_text("Open links to this site without asking (Settings)")
                    .clicked()
                {
                    app.trust_site(&q.site);
                }
                if ui.button("Skip").on_hover_text("The room goes on without you").clicked() {
                    app.send(UiCommand::SkipLink(q.item_id));
                }
            });
        });
    }
    if let (Some(room), Some(hold)) =
        (&snap.room, snap.playback.as_ref().and_then(|p| p.hold.as_ref()))
    {
        theme::callout(ui, YELLOW, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(waiting_line(hold, room, &snap.progress, snap.me));
                if let Some(ready) = ready_button(ui, room, hold, snap.me) {
                    app.send(UiCommand::SetReady(ready));
                }
                if snap.is_host && ui.button("Start without them").clicked() {
                    app.send(UiCommand::StartWithoutWaiting);
                }
            });
        });
    }
    if app.host.is_some() && app.config.host.media.is_empty() {
        theme::callout(ui, theme::ACCENT, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("Nothing is shared yet: add a folder in Settings → My media folders.");
                if ui.button("Open settings").clicked() {
                    app.ui.settings_open = true;
                }
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use egui::accesskit::Role;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use sppy_proto::*;

    /// The waiting line of a new item, as `me` (participant 1) sees it.
    fn waiting(ready_check: bool, reason: HoldReason, ready: bool) -> Harness<'static, Vec<bool>> {
        let room = RoomState {
            room_id: RoomId::main(),
            participants: vec![Participant {
                id: ParticipantId(1),
                nickname: "me".into(),
                is_host: false,
                can_control: false,
                connected: true,
                status: MediaStatus::Ready { duration_ms: None, title: None },
                ready,
            }],
            control_mode: ControlMode::Everyone,
            lag_policy: LagPolicy::default(),
            ready_check,
            repeat: Repeat::Off,
            playlist: vec![],
            settings: RoomSettings { stall_hold_secs: 10, resume_buffer_secs: 10 },
        };
        let hold = Hold { reason, waiting_for: vec![ParticipantId(1)] };
        let mut h = Harness::builder().with_size([300.0, 60.0]).build_ui_state(
            move |ui, pressed: &mut Vec<bool>| {
                super::theme::apply(ui.ctx());
                pressed.extend(super::ready_button(ui, &room, &hold, Some(ParticipantId(1))));
            },
            Vec::new(),
        );
        h.run();
        h
    }

    #[test]
    fn ready_is_ticked_and_taken_back() {
        let mut h = waiting(true, HoldReason::ItemStart, false);
        h.get_by_role_and_label(Role::CheckBox, "I'm ready").click();
        h.run();
        assert_eq!(h.state(), &[true]);
        let mut h = waiting(true, HoldReason::ItemStart, true);
        h.snapshot("ready_ticked");
        h.get_by_role_and_label(Role::CheckBox, "I'm ready").click();
        h.run();
        assert_eq!(h.state(), &[false]);
    }

    #[test]
    fn no_ready_tick_without_the_check_or_for_a_stall() {
        for (check, reason) in [(false, HoldReason::ItemStart), (true, HoldReason::Stalled)] {
            let h = waiting(check, reason, false);
            assert_eq!(h.query_all_by_role(Role::CheckBox).count(), 0, "{check} {reason:?}");
        }
    }

    #[test]
    fn version_text_names_the_app_and_the_protocol() {
        let t = super::version_text();
        assert!(t.starts_with(&format!("sppy {}", env!("CARGO_PKG_VERSION"))), "{t}");
        assert!(t.ends_with(&format!("protocol v{}", sppy_proto::PROTOCOL_VERSION)), "{t}");
    }
}
