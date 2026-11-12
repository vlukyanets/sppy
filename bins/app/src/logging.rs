//! Logs go to stderr and to a daily file in the data directory.

use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

pub fn init(logs_dir: &Path) -> Option<WorkerGuard> {
    let filter = || EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let stderr =
        tracing_subscriber::fmt::layer().with_writer(std::io::stderr).with_filter(filter());
    match std::fs::create_dir_all(logs_dir) {
        Ok(()) => {
            let (writer, guard) = tracing_appender::non_blocking(tracing_appender::rolling::daily(
                logs_dir, "sppy.log",
            ));
            let file = tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(writer)
                .with_filter(filter());
            tracing_subscriber::registry().with(stderr).with(file).init();
            Some(guard)
        }
        Err(e) => {
            tracing_subscriber::registry().with(stderr).init();
            tracing::warn!("cannot create {}: {e}", logs_dir.display());
            None
        }
    }
}
