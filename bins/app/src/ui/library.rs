//! The server's media library as a tree.

use sppy_client_core::api::{Snapshot, UiCommand};
use sppy_proto::{LibraryNode, Source};

use crate::app::SppyApp;

fn human_size(bytes: u64) -> String {
    let gb = bytes as f64 / (1u64 << 30) as f64;
    if gb >= 1.0 {
        format!("{gb:.1} GB")
    } else {
        format!("{:.0} MB", bytes as f64 / (1u64 << 20) as f64)
    }
}

fn node(app: &mut SppyApp, ui: &mut egui::Ui, n: &LibraryNode, can_add: bool, depth: usize) {
    match n {
        LibraryNode::Dir { name, children } => {
            egui::CollapsingHeader::new(name).id_salt((name, depth)).default_open(depth == 0).show(
                ui,
                |ui| {
                    for c in children {
                        node(app, ui, c, can_add, depth + 1);
                    }
                },
            );
        }
        LibraryNode::File { file_id, name, size, subtitles, .. } => {
            ui.horizontal(|ui| {
                if can_add && ui.small_button("+").on_hover_text("Add to the playlist").clicked() {
                    app.send(UiCommand::PlaylistAdd(vec![Source::File {
                        file_id: file_id.clone(),
                    }]));
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
    if snap.room.is_none() {
        return;
    }
    if !app.ui.library_requested {
        app.ui.library_requested = true;
        app.send(UiCommand::GetLibrary);
    }
    if snap.is_host && ui.small_button("Rescan").clicked() {
        app.send(UiCommand::RescanLibrary);
    }
    match &snap.library {
        None => {
            ui.weak("Loading…");
        }
        Some(roots) if roots.is_empty() => {
            ui.weak("The server has no media folders configured.");
        }
        Some(roots) => {
            let can_add = snap.can_control();
            for r in roots.clone() {
                node(app, ui, &r, can_add, 0);
            }
        }
    }
}
