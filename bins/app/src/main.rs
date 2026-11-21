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
use sppy_player::any::AnyLauncher;

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
    let (config, config_problems) = match ClientConfig::load_checked(&paths.config_file) {
        Ok(loaded) => loaded,
        Err(e) => {
            tracing::error!("{e}; using defaults");
            (ClientConfig::default(), Vec::new())
        }
    };
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("tokio runtime")?;
    let launcher = AnyLauncher::new(config.player.setup());
    let handle = {
        let _enter = rt.enter();
        spawn_client(config.clone(), launcher.clone())
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("sppy")
            .with_inner_size([980.0, 640.0])
            .with_min_inner_size([640.0, 420.0])
            .with_icon(window_icon()),
        ..Default::default()
    };
    eframe::run_native(
        "sppy",
        options,
        Box::new(move |cc| {
            let mut app = app::SppyApp::new(cc, rt, handle, launcher, config, paths, link);
            app.config_problems_found(config_problems);
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}

/// The title bar and taskbar icon. Wayland ignores it and takes the icon of the
/// installed `sppy.desktop` (the app id is "sppy").
fn window_icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(include_bytes!("../../../assets/sppy-256.png"))
        .expect("the icon is a valid PNG built into the binary")
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_window_icon_decodes() {
        let icon = super::window_icon();
        assert_eq!((icon.width, icon.height), (256, 256));
    }
}
