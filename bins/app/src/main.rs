//! sppy desktop app: a small egui window around `sppy-client-core`.

// Release builds on Windows are GUI apps: no console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod host;
mod instance;
mod links;
mod log;
mod logging;
mod search;
mod ui;

use anyhow::Context;
use sppy_client_core::api::spawn_client;
use sppy_client_core::config::{ClientConfig, client_paths};
use sppy_player::mpv::{MpvConfig, MpvLauncher};

fn main() -> anyhow::Result<()> {
    let paths = client_paths();
    // `sppy sppy://…` (a clicked invite link): hand it to the app if it is already open.
    let link = links::link_from_args(std::env::args().skip(1));
    if let Some(link) = &link
        && instance::forward(&paths.data_dir, link)
    {
        return Ok(());
    }
    let _log_guard = logging::init(&paths.logs_dir);
    let config = match ClientConfig::load_or_default(&paths.config_file) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("{e}; using defaults");
            ClientConfig::default()
        }
    };
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("tokio runtime")?;
    let launcher = MpvLauncher::new(MpvConfig {
        path: config.player.path.clone(),
        extra_args: config.player.extra_args.clone(),
        headless: false,
    });
    let handle = {
        let _enter = rt.enter();
        spawn_client(config.clone(), launcher.clone())
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("sppy")
            .with_inner_size([980.0, 640.0])
            .with_min_inner_size([640.0, 420.0]),
        ..Default::default()
    };
    eframe::run_native(
        "sppy",
        options,
        Box::new(move |cc| {
            Ok(Box::new(app::SppyApp::new(cc, rt, handle, launcher, config, paths, link)))
        }),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}
