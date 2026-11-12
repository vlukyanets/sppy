//! Headless sppy server.

use std::path::{Path, PathBuf};

use anyhow::Context;
use clap::{Parser, Subcommand};
use sppy_server::{ServerConfig, paths, tokens};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "sppy-server", version, about = "Headless sppy server")]
struct Cli {
    /// Config file (default: <config dir>/sppy/server.toml).
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    /// Data directory for tokens and state (default: <data dir>/sppy/server).
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the server (default).
    Run,
    /// Print the host and invite tokens.
    PrintTokens,
    /// Generate a new host token.
    RotateHostToken,
    /// Generate a new invite token.
    RotateInvite,
}

fn load_config(path: &Path) -> anyhow::Result<ServerConfig> {
    match std::fs::read_to_string(path) {
        Ok(text) => ServerConfig::from_toml(&text)
            .with_context(|| format!("invalid config {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::warn!("config {} not found, using defaults", path.display());
            Ok(ServerConfig::default())
        }
        Err(e) => Err(e).with_context(|| format!("cannot read {}", path.display())),
    }
}

fn print_tokens(t: &tokens::Tokens) {
    println!("host token:   {}", t.host);
    println!("invite token: {}", t.invite);
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    let cli = Cli::parse();
    let data_dir = cli.data_dir.unwrap_or_else(paths::default_data_dir);
    match cli.command.unwrap_or(Command::Run) {
        Command::PrintTokens => print_tokens(&tokens::load_or_create(&data_dir)?.0),
        Command::RotateHostToken => print_tokens(&tokens::rotate_host(&data_dir)?),
        Command::RotateInvite => print_tokens(&tokens::rotate_invite(&data_dir)?),
        Command::Run => {
            let config_path = cli.config.unwrap_or_else(paths::default_config_path);
            let config = load_config(&config_path)?;
            let handle = sppy_server::start(config, data_dir).await?;
            tokio::signal::ctrl_c().await.context("cannot wait for Ctrl-C")?;
            tracing::info!("shutting down");
            handle.shutdown().await;
        }
    }
    Ok(())
}
