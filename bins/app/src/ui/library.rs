//! The server's media library: search, tree, add a file or a whole folder.

use sppy_client_core::api::{Snapshot, UiCommand};
use sppy_proto::{LibraryNode, Source};

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
                ui.label(name);
                if can_add
                    && ui
                        .small_button("+ all")
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
            ui.horizontal(|ui| {
                if can_add && ui.small_button("+").on_hover_text("Add to the playlist").clicked() {
                    add_files(app, vec![file_id.clone()]);
                }
                ui.add(egui::Label::new(name).truncate());
                ui.weak(human_size(*size));
                if !subtitles.is_empty() {
                    ui.weak("subs");
                }
            });
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
                .desired_width(240.0),
        );
        if !app.ui.library_query.is_empty() && ui.small_button("✕").clicked() {
            app.ui.library_query.clear();
        }
        if snap.is_host && ui.small_button("Rescan").clicked() {
            app.send(UiCommand::RescanLibrary);
        }
    });
    let Some(roots) = snap.library.clone() else {
        ui.weak("Loading…");
        return;
    };
    if roots.is_empty() {
        if snap.is_host {
            ui.weak("No media folders: add one in Settings → Hosting, then Rescan.");
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
    if can_add
        && hits.len() > 1
        && ui.small_button(format!("+ all {} results", hits.len())).clicked()
    {
        add_files(app, hits.iter().map(|h| h.file_id.clone()).collect());
    }
    for h in hits {
        ui.horizontal(|ui| {
            if can_add && ui.small_button("+").clicked() {
                add_files(app, vec![h.file_id.clone()]);
            }
            ui.add(egui::Label::new(&h.name).truncate()).on_hover_text(&h.folder);
            ui.weak(human_size(h.size));
        });
    }
}
