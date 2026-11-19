//! Settings window. Saved to client.toml; player and sync settings apply on the next launch.

use std::path::PathBuf;

use sppy_client_core::api::{ClientHandle, ServerFolders, Snapshot, UiCommand};
use sppy_client_core::netaddr;
use sppy_common::folders;
use sppy_proto::{FolderAdding, MediaFolderProblem};

use super::theme;
use crate::app::{SppyApp, UiState};

/// A path that may be long: cut with "…" to the room left (full path on hover), so it
/// does not widen the window and push the hints out of sight.
fn path_label(ui: &mut egui::Ui, path: &str) {
    ui.add(egui::Label::new(path).truncate()).on_hover_text(path);
}

/// `ru, en` or `ru en` → `["ru", "en"]`.
fn language_list(text: &str) -> Vec<String> {
    text.split([',', ' ']).map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect()
}

/// Two columns: labels on the left, all of the same width.
fn form(ui: &mut egui::Ui, id: &str, add: impl FnOnce(&mut egui::Ui)) {
    egui::Grid::new(id).num_columns(2).min_col_width(150.0).spacing([12.0, 8.0]).show(ui, add);
}

fn wide_edit(ui: &mut egui::Ui, text: &mut String, hint: &str) -> egui::Response {
    ui.add(
        egui::TextEdit::singleline(text)
            .hint_text(hint)
            .margin(egui::Margin::symmetric(8, 4))
            .desired_width(360.0),
    )
}

fn pick_folder() -> Option<PathBuf> {
    rfd::FileDialog::new().pick_folder()
}

pub fn show(app: &mut SppyApp, ctx: &egui::Context, snap: &Snapshot) {
    if !app.ui.settings_open {
        // Looked up again the next time Settings open (it may change outside the app).
        app.ui.link_handler = None;
        return;
    }
    if app.ui.link_handler.is_none() {
        app.ui.link_handler = Some(crate::links::is_registered());
    }
    let mut open = app.ui.settings_open;
    let mut save = false;
    let mut restart = false;
    let mut rescan = false;
    let screen = ctx.content_rect();
    egui::Window::new("Settings")
        .open(&mut open)
        .collapsible(false)
        .vscroll(true)
        .default_size([680.0, (screen.height() - 120.0).clamp(360.0, 760.0)])
        .default_pos(screen.center())
        .pivot(egui::Align2::CENTER_CENTER)
        .show(ctx, |ui| {
            let c = &mut app.config;
            theme::card(ui, "Player", None, |ui| {
                form(ui, "player", |ui| {
                    ui.label("mpv path");
                    let mut path = c.player.path.display().to_string();
                    if wide_edit(ui, &mut path, "mpv").changed() {
                        c.player.path = path.into();
                    }
                    ui.end_row();
                    ui.label("Extra arguments");
                    if wide_edit(ui, &mut app.ui.extra_args, "e.g. --volume=70").changed() {
                        c.player.extra_args =
                            app.ui.extra_args.split_whitespace().map(str::to_owned).collect();
                    }
                    ui.end_row();
                    ui.label("Audio languages");
                    if wide_edit(ui, &mut app.ui.audio_languages, "e.g. ja, en (first one wins)")
                        .changed()
                    {
                        c.audio_languages = language_list(&app.ui.audio_languages);
                    }
                    ui.end_row();
                    ui.label("Subtitle languages");
                    if wide_edit(ui, &mut app.ui.subtitle_languages, "e.g. ru, en (first one wins)")
                        .changed()
                    {
                        c.subtitle_languages = language_list(&app.ui.subtitle_languages);
                    }
                    ui.end_row();
                });
                ui.horizontal(|ui| {
                    if ui.button("Restart player").clicked() {
                        restart = true;
                    }
                    theme::muted(ui, "Path and arguments apply when the player starts.");
                });
                if !c.trusted_sites.is_empty() {
                    ui.add_space(6.0);
                    theme::muted(ui, "Links to these sites play without asking (✕ to ask again):");
                    ui.horizontal_wrapped(|ui| {
                        let mut remove = None;
                        for (i, site) in c.trusted_sites.iter().enumerate() {
                            if ui.button(format!("{site}  ✕")).clicked() {
                                remove = Some(i);
                            }
                        }
                        if let Some(i) = remove {
                            c.trusted_sites.remove(i);
                        }
                    });
                }
            });

            let about = "Videos here play from your disk instead of being downloaded. When you \
                         host, everyone sees them in the Library under the name on the left.";
            theme::card(ui, "My media folders", Some(about), |ui| {
                let mut remove = None;
                if !c.host.media.is_empty() {
                    ui.horizontal(|ui| {
                        ui.add_sized(
                            [180.0, 14.0],
                            egui::Label::new(
                                egui::RichText::new("NAME IN LIBRARY").small().color(theme::MUTED),
                            ),
                        );
                        ui.label(egui::RichText::new("FOLDER").small().color(theme::MUTED));
                    });
                }
                for (i, r) in c.host.media.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut r.name)
                                .margin(egui::Margin::symmetric(8, 4))
                                .desired_width(172.0),
                        )
                        .on_hover_text("How this folder is called in the Library while you host");
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("✕").on_hover_text("Remove from the list").clicked() {
                                remove = Some(i);
                            }
                            ui.with_layout(
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    path_label(ui, &r.path.display().to_string());
                                },
                            );
                        });
                    });
                }
                if let Some(i) = remove {
                    c.host.media.remove(i);
                }
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.button("Add folder…").clicked()
                        && let Some(path) = pick_folder()
                    {
                        match folders::new_folder(&c.host.media, &path, None) {
                            Ok(root) => {
                                c.host.media.push(root);
                                app.ui.folder_error = None;
                            }
                            Err(MediaFolderProblem::AlreadyShared) => {
                                app.ui.folder_error =
                                    Some("This folder is already in the list".into());
                            }
                            Err(_) => app.ui.folder_error = Some("Cannot read this folder".into()),
                        }
                    }
                    if ui
                        .button("Rescan")
                        .on_hover_text("Look through the folders again, e.g. after adding files")
                        .clicked()
                    {
                        rescan = true;
                    }
                    match snap.local.indexed_files {
                        None => theme::muted(ui, "Looking for video files…"),
                        Some(n) => theme::muted(ui, &format!("{n} video files found")),
                    }
                });
                if let Some(e) = &app.ui.folder_error {
                    ui.colored_label(super::RED, e);
                }
            });

            if snap.is_host
                && app.host.is_none()
                && let Some(server) = &snap.server_folders
            {
                server_folders(&mut app.ui, &app.handle, ui, server);
            }

            if let Some(Some(registered)) = app.ui.link_handler {
                theme::card(ui, "Invite links", None, |ui| {
                    let (text, button) = if registered {
                        ("Clicking an sppy:// link opens this app with the link filled in.", "Stop")
                    } else {
                        (
                            "Clicking an sppy:// link in a browser or messenger can open this app \
                             with the link filled in.",
                            "Open sppy:// links with this app",
                        )
                    };
                    theme::muted(ui, text);
                    if ui.button(button).clicked() {
                        let result = if registered {
                            crate::links::unregister()
                        } else {
                            crate::links::register()
                        };
                        app.ui.link_handler_error = result.err();
                        app.ui.link_handler = None;
                    }
                    if let Some(e) = &app.ui.link_handler_error {
                        ui.colored_label(super::RED, e);
                    }
                });
            }

            theme::card(ui, "Downloads", None, |ui| {
                ui.checkbox(&mut c.prefetch_next, "Download the next playlist item in advance");
                form(ui, "downloads", |ui| {
                    ui.label("Cache limit");
                    ui.add(
                        egui::DragValue::new(&mut c.cache_limit_gb).range(1..=10_000).suffix(" GB"),
                    );
                    ui.end_row();
                    ui.label("In use");
                    ui.horizontal(|ui| {
                        ui.label(format!(
                            "{:.1} GB",
                            snap.local.cache_used_bytes as f64 / (1u64 << 30) as f64
                        ));
                        if ui.button("Clear cache").clicked() {
                            let _ = app.handle.commands.send(UiCommand::ClearCache);
                        }
                    });
                    ui.end_row();
                });
            });

            theme::card(ui, "Hosting", Some("Used when you click Become host."), |ui| {
                form(ui, "host", |ui| {
                    let h = &mut c.host;
                    ui.label("Listen on");
                    let options = netaddr::listen_options(&app.ui.local_ips, h.listen.ip());
                    let mut ip = h.listen.ip();
                    let current = options.iter().find(|(o, _)| *o == ip).map(|(_, l)| l.clone());
                    egui::ComboBox::from_id_salt("listen-ip")
                        .selected_text(current.unwrap_or_default())
                        .width(260.0)
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
                    if wide_edit(ui, &mut public, "e.g. mybox.tailnet.ts.net").changed() {
                        h.public_address = Some(public.trim().to_owned()).filter(|s| !s.is_empty());
                    }
                    ui.end_row();
                });
            });

            theme::card(ui, "Sync", None, |ui| {
                ui.collapsing("Advanced sync settings", |ui| {
                    let a = &mut c.advanced;
                    form(ui, "sync", |ui| {
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
            });

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.add(theme::primary("Save")).clicked() {
                    save = true;
                }
                theme::muted(
                    ui,
                    "Folders and languages apply on Save; sync and hosting on the next start.",
                );
            });
        });
    app.ui.settings_open = open;
    if save || restart {
        app.save_config();
    }
    if save {
        app.send(UiCommand::SetTrustedSites(app.config.trusted_sites.clone()));
        app.send(UiCommand::SetTrackLanguages(app.config.track_languages()));
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
    let about = "What the server shares with everyone. Paths are on the server's machine.";
    theme::card(ui, "Server folders", Some(about), |ui| {
        server_folder_list(state, &send, ui, server)
    });
}

fn server_folder_list(
    state: &mut UiState,
    send: &dyn Fn(UiCommand),
    ui: &mut egui::Ui,
    server: &ServerFolders,
) {
    for f in &server.folders {
        ui.horizontal(|ui| {
            if server.editable
                && state.confirm_unshare.as_deref() != Some(f.name.as_str())
                && ui.button("✕").on_hover_text("Stop sharing this folder").clicked()
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
                if ui.button("Yes").clicked() {
                    send(UiCommand::RemoveServerFolder { name: f.name.clone() });
                    state.confirm_unshare = None;
                }
                if ui.button("No").clicked() {
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

#[cfg(test)]
mod tests {
    use super::language_list;

    #[test]
    fn languages_split_on_commas_and_spaces() {
        assert_eq!(language_list(" ru, en  ja,,"), ["ru", "en", "ja"]);
        assert!(language_list("  ").is_empty());
    }
}
