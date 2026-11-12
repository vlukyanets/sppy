//! Settings window. Saved to client.toml; player and sync settings apply on the next launch.

use std::path::PathBuf;

use sppy_client_core::api::{Snapshot, UiCommand};
use sppy_common::config::MediaRoot;

use crate::app::SppyApp;

fn pick_folder() -> Option<PathBuf> {
    rfd::FileDialog::new().pick_folder()
}

pub fn show(app: &mut SppyApp, ctx: &egui::Context, snap: &Snapshot) {
    let mut open = app.ui.settings_open;
    let mut save = false;
    egui::Window::new("Settings").open(&mut open).vscroll(true).show(ctx, |ui| {
        let c = &mut app.config;
        ui.heading("Player");
        egui::Grid::new("player").num_columns(2).show(ui, |ui| {
            ui.label("mpv path");
            let mut path = c.player.path.display().to_string();
            if ui.text_edit_singleline(&mut path).changed() {
                c.player.path = path.into();
            }
            ui.end_row();
            ui.label("Extra arguments");
            if ui.text_edit_singleline(&mut app.ui.extra_args).changed() {
                c.player.extra_args =
                    app.ui.extra_args.split_whitespace().map(str::to_owned).collect();
            }
            ui.end_row();
        });

        ui.heading("My media folders");
        ui.weak("Files here are used instead of downloading when they match the server's copy.");
        let mut remove = None;
        for (i, p) in c.local_media.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(p.display().to_string());
                if ui.small_button("✕").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            c.local_media.remove(i);
        }
        if ui.button("Add folder…").clicked()
            && let Some(p) = pick_folder()
        {
            c.local_media.push(p);
        }

        ui.heading("Downloads");
        ui.checkbox(&mut c.prefetch_next, "Download the next playlist item in advance");
        ui.horizontal(|ui| {
            ui.label("Cache limit, GB");
            ui.add(egui::DragValue::new(&mut c.cache_limit_gb).range(1..=10_000));
        });
        ui.horizontal(|ui| {
            ui.label(format!(
                "In use: {:.1} GB",
                snap.local.cache_used_bytes as f64 / (1u64 << 30) as f64
            ));
            if ui.button("Clear cache").clicked() {
                let _ = app.handle.commands.send(UiCommand::ClearCache);
            }
        });

        ui.collapsing("Advanced sync", |ui| {
            let a = &mut c.advanced;
            egui::Grid::new("sync").num_columns(2).show(ui, |ui| {
                let row = |ui: &mut egui::Ui, label: &str, v: &mut u64| {
                    ui.label(label);
                    ui.add(egui::DragValue::new(v).suffix(" ms"));
                    ui.end_row();
                };
                row(ui, "Dead band", &mut a.dead_band_ms);
                row(ui, "Speed correction exit", &mut a.speed_exit_ms);
                row(ui, "Speed correction horizon", &mut a.horizon_ms);
                row(ui, "Seek threshold", &mut a.seek_threshold_ms);
                row(ui, "Settle time", &mut a.settle_ms);
                row(ui, "Max seek lead", &mut a.max_seek_lead_ms);
                row(ui, "Re-seek threshold", &mut a.reseek_threshold_ms);
                ui.label("Max speed correction");
                ui.add(
                    egui::DragValue::new(&mut a.max_speed_correction).range(0.0..=0.5).speed(0.005),
                );
                ui.end_row();
            });
        });

        ui.heading("Hosting (Become host)");
        egui::Grid::new("host").num_columns(2).show(ui, |ui| {
            let h = &mut c.host;
            ui.label("Port");
            let mut port = h.listen.port();
            if ui.add(egui::DragValue::new(&mut port).range(1..=65535)).changed() {
                h.listen.set_port(port);
            }
            ui.end_row();
            ui.label("Public address");
            let mut public = h.public_address.clone().unwrap_or_default();
            if ui
                .add(egui::TextEdit::singleline(&mut public).hint_text("e.g. mybox.tailnet.ts.net"))
                .changed()
            {
                h.public_address = Some(public.trim().to_owned()).filter(|s| !s.is_empty());
            }
            ui.end_row();
        });
        let mut remove = None;
        for (i, r) in c.host.media.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut r.name).desired_width(100.0));
                ui.label(r.path.display().to_string());
                if ui.small_button("✕").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            c.host.media.remove(i);
        }
        if ui.button("Add media folder…").clicked()
            && let Some(path) = pick_folder()
        {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Media".into());
            c.host.media.push(MediaRoot { name, path });
        }

        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("Save").clicked() {
                save = true;
            }
            ui.weak("Player, sync and hosting settings apply on the next start.");
        });
    });
    app.ui.settings_open = open;
    if save {
        app.save_config();
    }
}
