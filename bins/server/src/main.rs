//! Headless sppy server.

use std::net::IpAddr;
use std::path::{Path, PathBuf};

use anyhow::Context;
use clap::{Parser, Subcommand};
use sppy_common::link;
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

/// `(invite link, host link)`: to `public_address` if set, else to `fallback_ip` (this
/// machine's address) or the address the server is bound to.
fn links(
    config: &ServerConfig,
    t: &tokens::Tokens,
    fallback_ip: Option<IpAddr>,
) -> (String, String) {
    let bound = config.listen.ip();
    let ip = if bound.is_unspecified() { fallback_ip } else { Some(bound) };
    let link = |token: &str| {
        link::invite_link(config.public_address.as_deref(), config.listen.port(), token, ip)
    };
    (link(&t.invite), link(&t.host))
}

fn print_tokens(config: &ServerConfig, t: &tokens::Tokens) {
    let (invite, host) = links(config, t, link::first_non_loopback_ip());
    println!("host token:   {}", t.host);
    println!("invite token: {}", t.invite);
    println!();
    println!("Invite link (for friends):  {invite}");
    println!("Host link (keep it secret): {host}");
    if config.public_address.is_none() {
        println!();
        println!("If others reach this server by another name or address (e.g. a Tailscale");
        println!("name), set public_address in the config to put it into the links.");
    }
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
    let config_path = cli.config.unwrap_or_else(paths::default_config_path);
    let config = load_config(&config_path)?;
    match cli.command.unwrap_or(Command::Run) {
        Command::PrintTokens => print_tokens(&config, &tokens::load_or_create(&data_dir)?.0),
        Command::RotateHostToken => print_tokens(&config, &tokens::rotate_host(&data_dir)?),
        Command::RotateInvite => print_tokens(&config, &tokens::rotate_invite(&data_dir)?),
        Command::Run => {
            // Folders the host adds or removes from the app are saved into this file.
            let options = sppy_server::StartOptions {
                config_file: Some(config_path),
                require_media_root: true,
            };
            let handle = sppy_server::start_with(config, data_dir, options).await?;
            tokio::signal::ctrl_c().await.context("cannot wait for Ctrl-C")?;
            tracing::info!("shutting down");
            handle.shutdown().await;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens() -> tokens::Tokens {
        tokens::Tokens { host: "HOST".into(), invite: "INV".into() }
    }

    #[test]
    fn links_use_the_public_address() {
        let config = ServerConfig {
            public_address: Some("box.ts.net".into()),
            listen: "0.0.0.0:9000".parse().unwrap(),
            ..ServerConfig::default()
        };
        let lan = Some("192.168.1.5".parse().unwrap());
        assert_eq!(
            links(&config, &tokens(), lan),
            ("sppy://box.ts.net:9000/INV".into(), "sppy://box.ts.net:9000/HOST".into())
        );
    }

    #[test]
    fn links_fall_back_to_this_machine_or_the_bound_address() {
        let lan = Some("192.168.1.5".parse().unwrap());
        let config = ServerConfig::default();
        assert_eq!(links(&config, &tokens(), lan).0, "sppy://192.168.1.5:8740/INV");
        let bound = ServerConfig { listen: "100.64.0.7:8740".parse().unwrap(), ..config };
        assert_eq!(links(&bound, &tokens(), lan).1, "sppy://100.64.0.7:8740/HOST");
    }
}
