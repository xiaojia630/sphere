use anyhow::Result;
use clap::Parser;
use sphered::{Config, Daemon};
use std::path::PathBuf;

/// The Sphere service.
#[derive(Parser)]
#[command(version)]
struct Args {
    /// Config file (default: <state dir>/config.toml, if it exists).
    #[arg(long)]
    config: Option<PathBuf>,
    /// State directory (default: ~/.sphere).
    #[arg(long)]
    state_dir: Option<PathBuf>,
    /// API socket path.
    #[arg(long)]
    socket: Option<PathBuf>,
    /// sphere-host socket path.
    #[arg(long)]
    host_socket: Option<PathBuf>,
    /// The agent command (Claude Code).
    #[arg(long)]
    agent: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let args = Args::parse();
    let state_dir = args
        .state_dir
        .clone()
        .unwrap_or_else(sphere_proto::paths::default_state_dir);
    let config_path = args
        .config
        .clone()
        .unwrap_or_else(|| state_dir.join("config.toml"));
    let mut cfg = if config_path.exists() {
        Config::load(&config_path)?
    } else if args.config.is_some() {
        anyhow::bail!("config file {} does not exist", config_path.display());
    } else {
        Config::default()
    };
    if args.state_dir.is_some() {
        cfg.state_dir = state_dir;
    }
    if args.socket.is_some() {
        cfg.socket = args.socket;
    }
    if args.host_socket.is_some() {
        cfg.host_socket = args.host_socket;
    }
    if let Some(a) = args.agent {
        cfg.agent_command = a;
    }
    let daemon = Daemon::start(cfg).await?;
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
    tracing::info!("shutting down; agents keep running in sphere-host");
    daemon.shutdown().await;
    Ok(())
}
