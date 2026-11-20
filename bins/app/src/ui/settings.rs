//! Settings window. Saved to client.toml; player and sync settings apply on the next launch.

use std::collections::HashMap;
use std::path::PathBuf;

use sppy_client_core::api::{ClientHandle, ServerFolders, Snapshot, UiCommand};
use sppy_client_core::config::CatchUp;
use sppy_client_core::netaddr;
use sppy_common::folders;
use sppy_player::any::PlayerKind;
use sppy_proto::{FolderAdding, MediaFolderProblem};

use super::numbers::{self, Draft};
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

/// A plain text field for a number. The setting changes only while the text is valid;
/// otherwise the reason is shown next to the field.
fn number_field<T: Copy>(
    ui: &mut egui::Ui,
    drafts: &mut HashMap<&'static str, Draft>,
    key: &'static str,
    value: &mut T,
    show: impl Fn(T) -> String,
    parse: impl Fn(&str) -> Result<T, String>,
    unit: &str,
) {
    let draft = drafts.entry(key).or_insert_with(|| Draft { text: show(*value), error: None });
    ui.horizontal(|ui| {
        let mut edit = egui::TextEdit::singleline(&mut draft.text)
            .margin(egui::Margin::symmetric(8, 4))
            .desired_width(80.0);
        if draft.error.is_some() {
            edit = edit.text_color(super::RED);
        }
        if ui.add(edit).changed() {
            match parse(&draft.text) {
                Ok(v) => {
                    *value = v;
                    draft.error = None;
                }
                Err(e) => draft.error = Some(e),
            }
        }
        if !unit.is_empty() {
            ui.label(unit);
        }
        if let Some(e) = &draft.error {
            ui.colored_label(super::RED, e);
        }
    });
}

fn ms_field(
    ui: &mut egui::Ui,
    drafts: &mut HashMap<&'static str, Draft>,
    key: &'static str,
    value: &mut u64,
    min: u64,
) {
    number_field(
        ui,
        drafts,
        key,
        value,
        |v| v.to_string(),
        |t| numbers::whole(t, "ms", min, None),
        "ms",
    );
}

fn percent_field(
    ui: &mut egui::Ui,
    drafts: &mut HashMap<&'static str, Draft>,
    key: &'static str,
    value: &mut f64,
    min: f64,
    max: f64,
) {
    number_field(
        ui,
        drafts,
        key,
        value,
        numbers::percent_text,
        |t| numbers::percent(t, min, max),
        "%",
    );
}

fn catch_up_text(m: CatchUp) -> &'static str {
    match m {
        CatchUp::Speed => "Speed up or slow down",
        CatchUp::Jumps => "Jump",
        CatchUp::Off => "Leave my player alone",
    }
}

fn catch_up_hint(m: CatchUp) -> &'static str {
    match m {
        CatchUp::Speed => {
            "A few percent faster or slower until the player is back with the room; a jump \
             only when it is far off."
        }
        CatchUp::Jumps => {
            "The player jumps to the room when it is half a second off. For players that \
             cannot change speed, or when a change of tempo would be heard."
        }
        CatchUp::Off => {
            "sppy opens the items but does not play, pause or move your player, and what you \
             do in it stays there."
        }
    }
}

fn pick_folder() -> Option<PathBuf> {
    rfd::FileDialog::new().pick_folder()
}

pub fn show(app: &mut SppyApp, ctx: &egui::Context, snap: &Snapshot) {
    if !app.ui.settings_open {
        // Looked up again the next time Settings open (it may change outside the app).
        app.ui.link_handler = None;
        // Unsaved typos are not kept: the fields show the settings again next time.
        app.ui.number_drafts.clear();
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
                    ui.label("Player");
                    let mut kind = c.player.kind();
                    egui::ComboBox::from_id_salt("player-kind").selected_text(kind.name()).show_ui(
                        ui,
                        |ui| {
                            for k in PlayerKind::ALL {
                                ui.selectable_value(&mut kind, k, k.name());
                            }
                        },
                    );
                    if kind != c.player.kind() {
                        c.player.switch_to(kind);
                        app.ui.extra_args.clear();
                    }
                    ui.end_row();
                    ui.label(format!("{} path", kind.name()));
                    let mut path = c.player.path.display().to_string();
                    if wide_edit(ui, &mut path, kind.id()).changed() {
                        c.player.path = path.into();
                    }
                    ui.end_row();
                    ui.label("Extra arguments");
                    let hint = match kind {
                        PlayerKind::Mpv => "e.g. --volume=70",
                        PlayerKind::Vlc => "e.g. --no-sub-autodetect-file",
                    };
                    if wide_edit(ui, &mut app.ui.extra_args, hint).changed() {
                        c.player.extra_args =
                            app.ui.extra_args.split_whitespace().map(str::to_owned).collect();
                    }
                    ui.end_row();
                    ui.label("Catching up");
                    egui::ComboBox::from_id_salt("catch-up")
                        .selected_text(catch_up_text(c.catch_up))
                        .show_ui(ui, |ui| {
                            for m in [CatchUp::Speed, CatchUp::Jumps, CatchUp::Off] {
                                ui.selectable_value(&mut c.catch_up, m, catch_up_text(m));
                            }
                        })
                        .response
                        .on_hover_text(catch_up_hint(c.catch_up));
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
                    theme::muted(ui, "Save restarts the player when these change.");
                });
                if c.player.kind() == PlayerKind::Vlc {
                    theme::muted(
                        ui,
                        "sppy puts its control module (sppy.lua) into VLC's Lua folder. Web \
                         links need yt-dlp, as with mpv.",
                    );
                }
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
                    number_field(
                        ui,
                        &mut app.ui.number_drafts,
                        "cache_limit_gb",
                        &mut c.cache_limit_gb,
                        |v| v.to_string(),
                        |t| numbers::whole(t, "GB", 1, None),
                        "GB",
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
                    number_field(
                        ui,
                        &mut app.ui.number_drafts,
                        "port",
                        &mut port,
                        |v| v.to_string(),
                        |t| numbers::whole(t, "", 1, Some(65_535)).map(|p| p as u16),
                        "",
                    );
                    h.listen.set_port(port);
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
                    let drafts = &mut app.ui.number_drafts;
                    form(ui, "sync", |ui| {
                        let rows: [(&str, &'static str, &mut u64, u64); 7] = [
                            ("Dead band", "dead_band_ms", &mut a.dead_band_ms, 10),
                            ("Speed correction exit", "speed_exit_ms", &mut a.speed_exit_ms, 0),
                            ("Speed correction horizon", "horizon_ms", &mut a.horizon_ms, 1_000),
                            ("Seek threshold", "seek_threshold_ms", &mut a.seek_threshold_ms, 100),
                            ("Settle time", "settle_ms", &mut a.settle_ms, 0),
                            ("Max seek lead", "max_seek_lead_ms", &mut a.max_seek_lead_ms, 0),
                            (
                                "Re-seek threshold",
                                "reseek_threshold_ms",
                                &mut a.reseek_threshold_ms,
                                50,
                            ),
                        ];
                        for (label, key, value, min) in rows {
                            ui.label(label);
                            ms_field(ui, drafts, key, value, min);
                            ui.end_row();
                        }
                        ui.label("Min speed correction");
                        percent_field(
                            ui,
                            drafts,
                            "min_speed",
                            &mut a.min_speed_correction,
                            0.0,
                            20.0,
                        );
                        ui.end_row();
                        ui.label("Max speed correction");
                        percent_field(
                            ui,
                            drafts,
                            "max_speed",
                            &mut a.max_speed_correction,
                            1.0,
                            50.0,
                        );
                        ui.end_row();
                    });
                });
            });

            ui.add_space(4.0);
            let typo = app.ui.number_drafts.values().any(|d| d.error.is_some());
            let conflict = numbers::sync_problems(&app.config.advanced).first().copied();
            ui.horizontal(|ui| {
                if ui.add_enabled(!typo && conflict.is_none(), theme::primary("Save")).clicked() {
                    save = true;
                }
                if typo {
                    ui.colored_label(super::RED, "Fix the values in red to save.");
                } else if let Some(conflict) = conflict {
                    ui.colored_label(super::RED, format!("Advanced sync settings: {conflict}"));
                } else {
                    theme::muted(
                        ui,
                        "Folders, languages and catching up apply on Save; advanced sync and \
                         hosting on the next start.",
                    );
                }
            });
        });
    app.ui.settings_open = open;
    if save && app.config.player.setup() != app.player_setup {
        restart = true;
    }
    if save || restart {
        app.save_config();
    }
    if save {
        app.send(UiCommand::SetCatchUp(app.config.catch_up));
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
