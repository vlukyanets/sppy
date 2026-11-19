//! Settings window. Saved to client.toml; player and sync settings apply on the next launch.

use std::path::PathBuf;

use sppy_client_core::api::{ClientHandle, ServerFolders, Snapshot, UiCommand};
use sppy_client_core::netaddr;
use sppy_common::folders;
use sppy_proto::{FolderAdding, MediaFolderProblem};

use crate::app::{SppyApp, UiState};

/// A path that may be long: cut with "…" to the room left (full path on hover), so it
/// does not widen the window and push the hints out of sight.
fn path_label(ui: &mut egui::Ui, path: &str) {
    ui.add(egui::Label::new(path).truncate()).on_hover_text(path);
}

fn pick_folder() -> Option<PathBuf> {
    rfd::FileDialog::new().pick_folder()
}

pub fn show(app: &mut SppyApp, ctx: &egui::Context, snap: &Snapshot) {
    let mut open = app.ui.settings_open;
    let mut save = false;
    let mut restart = false;
    let mut rescan = false;
    egui::Window::new("Settings").open(&mut open).vscroll(true).default_width(480.0).show(
        ctx,
        |ui| {
            let c = &mut app.config;
            ui.horizontal(|ui| {
                ui.heading("Player");
                if ui
                    .small_button("Restart player")
                    .on_hover_text("Apply the player settings now")
                    .clicked()
                {
                    restart = true;
                }
            });
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

            ui.horizontal(|ui| {
                ui.label("Subtitle languages");
                let r = ui.add(
                    egui::TextEdit::singleline(&mut app.ui.subtitle_languages)
                        .hint_text("e.g. ru, en (first one wins)"),
                );
                if r.changed() {
                    c.subtitle_languages = app
                        .ui
                        .subtitle_languages
                        .split([',', ' '])
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                        .collect();
                }
            });

            if !c.trusted_sites.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    ui.label("Links that play without asking:")
                        .on_hover_text("Besides YouTube, Vimeo, Twitch and other video sites");
                    let mut remove = None;
                    for (i, site) in c.trusted_sites.iter().enumerate() {
                        if ui.small_button(format!("{site} ✕")).on_hover_text("Ask again").clicked()
                        {
                            remove = Some(i);
                        }
                    }
                    if let Some(i) = remove {
                        c.trusted_sites.remove(i);
                    }
                });
            }

            ui.heading("My media folders");
            ui.weak(
                "Videos here play from your disk instead of being downloaded. \
             When you host, everyone can watch them.",
            );
            ui.horizontal(|ui| {
                match snap.local.indexed_files {
                    None => ui.weak("Looking for video files…"),
                    Some(n) => ui.weak(format!("Video files found: {n}")),
                };
                if ui
                    .small_button("Rescan")
                    .on_hover_text("Look through the folders again, e.g. after adding files")
                    .clicked()
                {
                    rescan = true;
                }
            });
            let mut remove = None;
            for (i, r) in c.host.media.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    if ui.small_button("✕").on_hover_text("Remove from the list").clicked() {
                        remove = Some(i);
                    }
                    ui.add(egui::TextEdit::singleline(&mut r.name).desired_width(100.0))
                        .on_hover_text("Name shown in the Library while you host");
                    path_label(ui, &r.path.display().to_string());
                });
            }
            if let Some(i) = remove {
                c.host.media.remove(i);
            }
            if ui.button("Add folder…").clicked()
                && let Some(path) = pick_folder()
            {
                match folders::new_folder(&c.host.media, &path, None) {
                    Ok(root) => {
                        c.host.media.push(root);
                        app.ui.folder_error = None;
                    }
                    Err(MediaFolderProblem::AlreadyShared) => {
                        app.ui.folder_error = Some("This folder is already in the list".into());
                    }
                    Err(_) => app.ui.folder_error = Some("Cannot read this folder".into()),
                }
            }
            if let Some(e) = &app.ui.folder_error {
                ui.colored_label(super::RED, e);
            }

            if snap.is_host
                && app.host.is_none()
                && let Some(server) = &snap.server_folders
            {
                server_folders(&mut app.ui, &app.handle, ui, server);
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
                    ui.label("Min speed correction");
                    ui.add(
                        egui::DragValue::new(&mut a.min_speed_correction)
                            .range(0.0..=0.2)
                            .speed(0.005),
                    );
                    ui.end_row();
                    ui.label("Max speed correction");
                    ui.add(
                        egui::DragValue::new(&mut a.max_speed_correction)
                            .range(0.0..=0.5)
                            .speed(0.005),
                    );
                    ui.end_row();
                });
            });

            ui.heading("Hosting (Become host)");
            egui::Grid::new("host").num_columns(2).show(ui, |ui| {
                let h = &mut c.host;
                ui.label("Listen on");
                let options = netaddr::listen_options(&app.ui.local_ips, h.listen.ip());
                let mut ip = h.listen.ip();
                let current = options.iter().find(|(o, _)| *o == ip).map(|(_, l)| l.clone());
                egui::ComboBox::from_id_salt("listen-ip")
                    .selected_text(current.unwrap_or_default())
                    .show_ui(ui, |ui| {
                        for (o, label) in &options {
                            ui.selectable_value(&mut ip, *o, label);
                        }
                    })
                    .response
                    .on_hover_text(
                        "Where the server accepts connections; all addresses by default",
                    );
                h.listen.set_ip(ip);
                ui.end_row();
                ui.label("Port");
                let mut port = h.listen.port();
                if ui.add(egui::DragValue::new(&mut port).range(1..=65535)).changed() {
                    h.listen.set_port(port);
                }
                ui.end_row();
                ui.label("Public address");
                let mut public = h.public_address.clone().unwrap_or_default();
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut public)
                            .hint_text("e.g. mybox.tailnet.ts.net"),
                    )
                    .changed()
                {
                    h.public_address = Some(public.trim().to_owned()).filter(|s| !s.is_empty());
                }
                ui.end_row();
            });
            ui.separator();
            if ui.button("Save").clicked() {
                save = true;
            }
            ui.weak(
                "Folders apply on Save; sync and hosting settings on the next start; \
             the player on Restart player.",
            );
        },
    );
    app.ui.settings_open = open;
    if save || restart {
        app.save_config();
    }
    if save {
        app.send(UiCommand::SetTrustedSites(app.config.trusted_sites.clone()));
        app.send(UiCommand::SetSubtitleLanguages(app.config.subtitle_languages.clone()));
    }
    if save || rescan {
        app.apply_media_folders(rescan);
    }
    if save && let Some(host) = &app.host {
        host.set_media_folders(app.config.host.media.clone());
    }
    if restart {
        app.restart_player();
    }
}

/// The folders of the server we are the host of (it runs elsewhere): paths on that machine.
fn server_folders(
    state: &mut UiState,
    handle: &ClientHandle,
    ui: &mut egui::Ui,
    server: &ServerFolders,
) {
    let send = |cmd| {
        let _ = handle.commands.send(cmd);
    };
    ui.heading("Server folders");
    ui.weak("What the server shares with everyone. Paths are on the server's machine.");
    for f in &server.folders {
        ui.horizontal(|ui| {
            if server.editable
                && state.confirm_unshare.as_deref() != Some(f.name.as_str())
                && ui.small_button("✕").on_hover_text("Stop sharing this folder").clicked()
            {
                state.confirm_unshare = Some(f.name.clone());
            }
            ui.strong(&f.name);
            if !f.available {
                ui.colored_label(super::RED, "not found on the server");
            }
            path_label(ui, &f.path);
        });
        if server.editable && state.confirm_unshare.as_deref() == Some(f.name.as_str()) {
            ui.horizontal(|ui| {
                ui.label(format!("Stop sharing {} for everyone?", f.name));
                if ui.small_button("Yes").clicked() {
                    send(UiCommand::RemoveServerFolder { name: f.name.clone() });
                    state.confirm_unshare = None;
                }
                if ui.small_button("No").clicked() {
                    state.confirm_unshare = None;
                }
            });
        }
    }
    if !server.editable {
        ui.weak("Changing them from the app is turned off in the server's server.toml.");
        return;
    }
    let hint = match &server.adding {
        FolderAdding::NoMediaRoot => {
            ui.weak("To share more folders from here, set media_root in the server's server.toml.");
            return;
        }
        FolderAdding::Within { root } => {
            ui.weak(format!("New folders must be inside {root}"));
            std::path::Path::new(root).join("Movies").to_string_lossy().into_owned()
        }
        FolderAdding::Anywhere => "/srv/media/Movies".to_owned(),
    };
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut state.server_folder_path)
                .hint_text(hint)
                .desired_width(220.0),
        );
        ui.add(
            egui::TextEdit::singleline(&mut state.server_folder_name)
                .hint_text("Name (optional)")
                .desired_width(110.0),
        );
        let path = state.server_folder_path.trim().to_owned();
        if ui
            .add_enabled(!path.is_empty(), egui::Button::new("Share"))
            .on_hover_text("Share this folder of the server's machine")
            .clicked()
        {
            let name = Some(state.server_folder_name.trim().to_owned()).filter(|n| !n.is_empty());
            send(UiCommand::AddServerFolder { path, name });
            state.server_folder_path.clear();
            state.server_folder_name.clear();
        }
    });
}
