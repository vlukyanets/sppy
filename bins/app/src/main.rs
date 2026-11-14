//! sppy desktop app: a small egui window around `sppy-client-core`.

mod app;
mod host;
mod logging;
mod ui;

use anyhow::Context;
use sppy_client_core::api::spawn_client;
use sppy_client_core::config::{ClientConfig, client_paths};
use sppy_player::mpv::{MpvConfig, MpvLauncher};

fn main() -> anyhow::Result<()> {
    let paths = client_paths();
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
        spawn_client(config.clone(), launcher)
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("sppy")
            .with_inner_size([420.0, 640.0]),
        ..Default::default()
    };
    eframe::run_native(
        "sppy",
        options,
        Box::new(move |cc| Ok(Box::new(app::SppyApp::new(cc, rt, handle, config, paths)))),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}
