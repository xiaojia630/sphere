//! sphered's link to sphere-host: a cached client that starts the host when
//! it is not running and retries once after a dropped connection.

use crate::config::Config;
use anyhow::Result;
use serde_json::Value;
use sphere_host::{ChildInfo, Follow, HostClient, SpawnParams, SpawnResult, Stream};
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct HostLink {
    cfg: Arc<Config>,
    client: Mutex<Option<HostClient>>,
}

/// Errors reported by the host itself start with `host: `; anything else means
/// the connection is gone.
fn is_transport(e: &anyhow::Error) -> bool {
    !format!("{e}").starts_with("host: ")
}

impl HostLink {
    pub fn new(cfg: Arc<Config>) -> HostLink {
        HostLink {
            cfg,
            client: Mutex::new(None),
        }
    }

    /// Connect, starting the host (detached) if nothing answers.
    pub async fn recover(&self) -> Result<HostClient> {
        let c = sphere_host::ensure_running(
            &self.cfg.host_binary(),
            &self.cfg.host_socket_path(),
            &self.cfg.host_dir(),
        )
        .await?;
        *self.client.lock().await = Some(c.clone());
        Ok(c)
    }

    pub async fn get(&self) -> Result<HostClient> {
        if let Some(c) = self.client.lock().await.clone() {
            return Ok(c);
        }
        self.recover().await
    }

    pub async fn pid(&self) -> Result<u32> {
        Ok(self.get().await?.ping().await?.pid)
    }

    pub async fn spawn(&self, p: SpawnParams) -> Result<SpawnResult> {
        match self.get().await?.spawn(p.clone()).await {
            Err(e) if is_transport(&e) => self.recover().await?.spawn(p).await,
            other => other,
        }
    }

    pub async fn write(&self, child: &str, bytes: &[u8]) -> Result<()> {
        match self.get().await?.write(child, bytes).await {
            Err(e) if is_transport(&e) => self.recover().await?.write(child, bytes).await,
            other => other,
        }
    }

    pub async fn close_stdin(&self, child: &str) -> Result<()> {
        match self.get().await?.close_stdin(child).await {
            Err(e) if is_transport(&e) => self.recover().await?.close_stdin(child).await,
            other => other,
        }
    }

    pub async fn signal(&self, child: &str, sig: Value, group: bool) -> Result<()> {
        match self.get().await?.signal(child, sig.clone(), group).await {
            Err(e) if is_transport(&e) => self.recover().await?.signal(child, sig, group).await,
            other => other,
        }
    }

    pub async fn list(&self) -> Result<Vec<ChildInfo>> {
        match self.get().await?.list().await {
            Err(e) if is_transport(&e) => self.recover().await?.list().await,
            other => other,
        }
    }

    pub async fn read(
        &self,
        child: &str,
        stream: Stream,
        from: u64,
        max: Option<usize>,
    ) -> Result<sphere_host::DecodedRead> {
        match self.get().await?.read(child, stream, from, max).await {
            Err(e) if is_transport(&e) => {
                self.recover().await?.read(child, stream, from, max).await
            }
            other => other,
        }
    }

    pub async fn follow(&self, child: &str, stream: Stream, from: u64) -> Result<Follow> {
        self.get().await?.follow(child, stream, from).await
    }
}
