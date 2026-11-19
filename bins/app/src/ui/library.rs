//! The server's media library: search, tree, add a file or a whole folder.

use sppy_client_core::api::{Snapshot, UiCommand};
use sppy_proto::{LibraryNode, Source};

use super::theme;
use crate::app::SppyApp;
use crate::search::{files_under, search};

fn human_size(bytes: u64) -> String {
    let gb = bytes as f64 / (1u64 << 30) as f64;
    if gb >= 1.0 {
        format!("{gb:.1} GB")
    } else {
        format!("{:.0} MB", bytes as f64 / (1u64 << 20) as f64)
    }
}

fn add_files(app: &SppyApp, ids: Vec<sppy_proto::FileId>) {
    if !ids.is_empty() {
        app.send(UiCommand::PlaylistAdd(
            ids.into_iter().map(|file_id| Source::File { file_id }).collect(),
        ));
    }
}

#[allow(clippy::too_many_arguments)]
fn file_row(
    app: &SppyApp,
    ui: &mut egui::Ui,
    file_id: &sppy_proto::FileId,
    name: &str,
    folder: Option<&str>,
    size: u64,
    subs: bool,
    can_add: bool,
) {
    let id = ui.scope_id().with(("library-file", file_id));
    theme::row(ui, id, false, |ui, hovered| {
        let add = can_add
            && ui
                .add(
                    egui::Button::new(egui::RichText::new("+").strong().color(if hovered {
                        theme::ACCENT
                    } else {
                        theme::MUTED
                    }))
                    .frame(false),
                )
                .on_hover_text("Add to the playlist")
                .clicked();
        let r = ui.add(egui::Label::new(name).truncate().sense(egui::Sense::click()));
        let r = match folder {
            Some(f) => r.on_hover_text(f),
            None => r,
        };
        if add || (can_add && r.double_clicked()) {
            add_files(app, vec![file_id.clone()]);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(egui::RichText::new(human_size(size)).small().color(theme::MUTED));
            if subs {
                theme::pill(ui, "subs", theme::MUTED);
            }
        });
    });
}

fn node(app: &mut SppyApp, ui: &mut egui::Ui, n: &LibraryNode, can_add: bool, depth: usize) {
    match n {
        LibraryNode::Dir { name, children } => {
            let id = ui.make_persistent_id((name, depth));
            egui::collapsing_header::CollapsingState::load_with_default_open(
                ui.ctx(),
                id,
                depth == 0,
            )
            .show_header(ui, |ui| {
                ui.label(egui::RichText::new(name).strong());
                if can_add
                    && ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new("+ add all").small().color(theme::ACCENT),
                            )
                            .frame(false),
                        )
                        .on_hover_text("Add every file in this folder")
                        .clicked()
                {
                    add_files(app, files_under(n));
                }
            })
            .body(|ui| {
                for c in children {
                    node(app, ui, c, can_add, depth + 1);
                }
            });
        }
        LibraryNode::File { file_id, name, size, subtitles, .. } => {
            file_row(app, ui, file_id, name, None, *size, !subtitles.is_empty(), can_add);
        }
    }
}

pub fn show(app: &mut SppyApp, ui: &mut egui::Ui, snap: &Snapshot) {
    if !app.ui.library_requested {
        app.ui.library_requested = true;
        app.send(UiCommand::GetLibrary);
    }
    let can_add = snap.can_control();
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut app.ui.library_query)
                .hint_text("Search")
                .margin(egui::Margin::symmetric(8, 5))
                .desired_width(280.0),
        );
        if !app.ui.library_query.is_empty() && ui.button("✕").clicked() {
            app.ui.library_query.clear();
        }
        if snap.is_host && ui.button("Rescan").clicked() {
            app.send(UiCommand::RescanLibrary);
            // The library folders are also where this computer finds its local copies.
            app.apply_media_folders(true);
        }
    });
    let Some(roots) = snap.library.clone() else {
        ui.weak("Loading…");
        return;
    };
    if roots.is_empty() {
        if snap.is_host && app.host.is_some() {
            ui.weak("Nothing shared yet: add a folder in Settings → My media folders, then Save.");
        } else if snap.is_host {
            ui.weak("Nothing shared yet: add a folder in Settings → Server folders.");
        } else {
            ui.weak("The host has not shared any media folders.");
        }
        return;
    }
    if app.ui.library_query.trim().is_empty() {
        for r in &roots {
            node(app, ui, r, can_add, 0);
        }
        return;
    }
    let hits = search(&roots, &app.ui.library_query);
    if hits.is_empty() {
        ui.weak("Nothing found.");
    }
    if can_add && hits.len() > 1 && ui.button(format!("+ all {} results", hits.len())).clicked() {
        add_files(app, hits.iter().map(|h| h.file_id.clone()).collect());
    }
    for h in hits {
        file_row(app, ui, &h.file_id, &h.name, Some(&h.folder), h.size, false, can_add);
    }
}
