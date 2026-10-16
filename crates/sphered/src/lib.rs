//! sphered: the Sphere service. See docs/sphered.md.

pub mod agent_proto;
pub mod config;
pub mod host_link;
pub mod hub;
pub mod manager;
pub mod peer;
pub mod policy;
pub mod server;
pub mod sessions;
pub mod store;
pub mod workspace;

use anyhow::{bail, Context, Result};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::Path;
use std::sync::Arc;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::watch;

pub use config::Config;

/// Create a directory only the owner can enter.
pub fn private_dir(path: &Path) -> Result<()> {
    if !path.exists() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)
            .with_context(|| format!("create {}", path.display()))?;
    }
    Ok(())
}

/// A running service. Dropping it or calling [`Daemon::shutdown`] stops the
/// API and the readers; the host and the agents keep running.
pub struct Daemon {
    pub manager: Arc<manager::Manager>,
    pub socket: std::path::PathBuf,
    shutdown_tx: watch::Sender<bool>,
    join: tokio::task::JoinHandle<()>,
}

impl Daemon {
    pub async fn start(cfg: Config) -> Result<Daemon> {
        private_dir(&cfg.state_dir)?;
        let socket = cfg.socket_path();
        if let Some(dir) = socket.parent() {
            private_dir(dir)?;
        }
        if socket.exists() {
            if UnixStream::connect(&socket).await.is_ok() {
                bail!("sphered is already running on {}", socket.display());
            }
            std::fs::remove_file(&socket)?;
        }
        let cfg = Arc::new(cfg);
        let hub = Arc::new(hub::Hub::open(&cfg.db_path(), cfg.subscriber_queue)?);
        let mgr = manager::Manager::new(cfg.clone(), hub);
        mgr.start().await?;
        let listener = UnixListener::bind(&socket).with_context(|| format!("bind {}", socket.display()))?;
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let join = tokio::spawn(server::serve(mgr.clone(), listener, shutdown_rx));
        tracing::info!(socket = %socket.display(), "sphered ready");
        Ok(Daemon { manager: mgr, socket, shutdown_tx, join })
    }

    /// Stop serving. Agents are not touched.
    pub async fn shutdown(self) {
        let _ = self.shutdown_tx.send(true);
        let _ = self.join.await;
        self.manager.abort_all();
        let _ = std::fs::remove_file(&self.socket);
    }
}
