//! Settings window. Saved to client.toml; player and sync settings apply on the next launch.

use std::collections::HashMap;
use std::path::PathBuf;

use sppy_client_core::api::{ClientHandle, ServerFolders, Snapshot, UiCommand};
use sppy_client_core::config::{
    CatchUp, MAX_SPEED_CORRECTION_RANGE, MIN_SPEED_CORRECTION_RANGE, MsField, SyncParams,
};
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

/// A form row with a plain text field for a number, named by its label (also for screen
/// readers; the label keys its draft). The setting changes only while the text is valid;
/// otherwise the reason is shown next to the field.
fn number_field<T: Copy>(
    ui: &mut egui::Ui,
    label: &'static str,
    drafts: &mut HashMap<&'static str, Draft>,
    value: &mut T,
    show: impl Fn(T) -> String,
    parse: impl Fn(&str) -> Result<T, String>,
    unit: &str,
) {
    let label_id = ui.label(label).id;
    let draft = drafts.entry(label).or_insert_with(|| Draft { text: show(*value), error: None });
    ui.horizontal(|ui| {
        let mut edit = egui::TextEdit::singleline(&mut draft.text)
            .margin(egui::Margin::symmetric(8, 4))
            .desired_width(80.0);
        if draft.error.is_some() {
            edit = edit.text_color(super::RED);
        }
        if ui.add(edit).labelled_by(label_id).changed() {
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
    label: &'static str,
    drafts: &mut HashMap<&'static str, Draft>,
    value: &mut u64,
    (min, max): (u64, Option<u64>),
) {
    number_field(
        ui,
        label,
        drafts,
        value,
        |v| v.to_string(),
        |t| numbers::whole(t, "ms", min, max),
        "ms",
    );
}

fn percent_field(
    ui: &mut egui::Ui,
    label: &'static str,
    drafts: &mut HashMap<&'static str, Draft>,
    value: &mut f64,
    range: (f64, f64),
) {
    number_field(
        ui,
        label,
        drafts,
        value,
        numbers::percent_text,
        |t| numbers::percent(t, range),
        "%",
    );
}

/// One text field per sync threshold.
fn advanced_sync(ui: &mut egui::Ui, drafts: &mut HashMap<&'static str, Draft>, a: &mut SyncParams) {
    form(ui, "sync", |ui| {
        for f in MsField::ALL {
            ms_field(ui, f.label(), drafts, f.get_mut(a), f.limits());
            ui.end_row();
        }
        let (min, max) = (MIN_SPEED_CORRECTION_RANGE, MAX_SPEED_CORRECTION_RANGE);
        percent_field(ui, "Min speed correction", drafts, &mut a.min_speed_correction, min);
        ui.end_row();
        percent_field(ui, "Max speed correction", drafts, &mut a.max_speed_correction, max);
        ui.end_row();
    });
    theme::muted(
        ui,
        "Jump threshold is used when catching up by jumps; the dead band and the speed \
         settings when catching up by speed.",
    );
}

/// Why Save is off: a field with text that is no valid value, or thresholds that break
/// their rules.
fn save_blocker(drafts: &HashMap<&'static str, Draft>, advanced: &SyncParams) -> Option<String> {
    if drafts.values().any(|d| d.error.is_some()) {
        return Some("Fix the values in red to save.".into());
    }
    let problem = advanced.problems().into_iter().next()?;
    Some(format!("Advanced sync settings: {}", problem.message))
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
                    number_field(
                        ui,
                        "Cache limit",
                        &mut app.ui.number_drafts,
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
                    let mut port = h.listen.port();
                    number_field(
                        ui,
                        "Port",
                        &mut app.ui.number_drafts,
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
                for p in &app.ui.config_problems {
                    ui.colored_label(super::YELLOW, p);
                }
                ui.collapsing("Advanced sync settings", |ui| {
                    advanced_sync(ui, &mut app.ui.number_drafts, &mut c.advanced);
                });
            });

            ui.add_space(4.0);
            let blocker = save_blocker(&app.ui.number_drafts, &app.config.advanced);
            ui.horizontal(|ui| {
                if ui.add_enabled(blocker.is_none(), theme::primary("Save")).clicked() {
                    save = true;
                }
                if let Some(blocker) = blocker {
                    ui.colored_label(super::RED, blocker);
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
    use super::*;
    use egui::accesskit::Role;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    struct Sync {
        drafts: HashMap<&'static str, Draft>,
        params: SyncParams,
    }

    fn advanced() -> Harness<'static, Sync> {
        let state = Sync { drafts: HashMap::new(), params: SyncParams::default() };
        let mut h = Harness::builder().with_size([520.0, 480.0]).build_ui_state(
            |ui, s: &mut Sync| {
                theme::apply(ui.ctx());
                theme::card(ui, "Sync", None, |ui| advanced_sync(ui, &mut s.drafts, &mut s.params));
            },
            state,
        );
        h.run();
        h
    }

    fn field<'h>(h: &'h Harness<'static, Sync>, label: &'h str) -> egui_kittest::Node<'h> {
        h.get_by_role_and_label(Role::TextInput, label)
    }

    #[test]
    fn thresholds_are_text_fields_not_drag_values() {
        let h = advanced();
        assert_eq!(h.query_all_by_role(Role::SpinButton).count(), 0, "no drag values");
        assert_eq!(h.query_all_by_role(Role::TextInput).count(), MsField::ALL.len() + 2);
        for f in MsField::ALL {
            let value = f.get(&SyncParams::default()).to_string();
            assert_eq!(field(&h, f.label()).value().as_deref(), Some(value.as_str()));
        }
        assert_eq!(field(&h, "Max speed correction").value().as_deref(), Some("8"));
    }

    #[test]
    fn a_typo_keeps_the_value_and_blocks_save() {
        let mut h = advanced();
        let dead_band = field(&h, "Dead band");
        dead_band.focus();
        dead_band.type_text("x");
        h.run();
        assert_eq!(h.state().params.dead_band_ms, 100, "the setting keeps its value");
        h.get_by_label("Enter a whole number");
        assert_eq!(
            save_blocker(&h.state().drafts, &h.state().params).as_deref(),
            Some("Fix the values in red to save.")
        );
    }

    #[test]
    fn a_valid_value_is_taken_and_a_conflict_named() {
        let mut h = advanced();
        let exit = field(&h, "Speed correction exit");
        exit.focus();
        exit.type_text("0");
        h.run();
        assert_eq!(h.state().params.speed_exit_ms, 300);
        assert_eq!(
            save_blocker(&h.state().drafts, &h.state().params).as_deref(),
            Some("Advanced sync settings: Speed correction exit must be below the dead band.")
        );
    }

    #[test]
    fn a_value_over_its_limit_names_the_limit() {
        let mut h = advanced();
        let settle = field(&h, "Settle time");
        settle.focus();
        settle.type_text("00");
        h.run();
        h.get_by_label("From 0 to 10000");
        assert_eq!(h.state().params.settle_ms, 1000);
    }

    #[test]
    fn advanced_sync_looks_right() {
        let mut h = advanced();
        let dead_band = field(&h, "Dead band");
        dead_band.focus();
        dead_band.type_text("x");
        h.run();
        h.fit_contents();
        h.snapshot("advanced_sync_with_a_typo");
    }

    #[test]
    fn languages_split_on_commas_and_spaces() {
        assert_eq!(language_list(" ru, en  ja,,"), ["ru", "en", "ja"]);
        assert!(language_list("  ").is_empty());
    }
}
