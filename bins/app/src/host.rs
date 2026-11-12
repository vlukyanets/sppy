//! "Become host": run the embedded server inside the app.

use std::path::PathBuf;

use sppy_common::config::ServerConfig;
use sppy_server::{ServerError, ServerHandle};

pub async fn become_host(
    cfg: ServerConfig,
    data_dir: PathBuf,
) -> Result<ServerHandle, ServerError> {
    sppy_server::start(cfg, data_dir).await
}
