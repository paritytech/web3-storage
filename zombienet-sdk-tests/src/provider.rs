//! Provider node lifecycle management.
//!
//! Spawns the storage provider as a child process and waits for it to become healthy.

use crate::config::*;
use anyhow::{anyhow, Result};
use std::time::Duration;
use tokio::process::{Child, Command};

/// A managed provider node process.
///
/// The provider is killed when this struct is dropped.
pub struct ProviderProcess {
    child: Child,
}

impl ProviderProcess {
    /// Spawn the storage provider node.
    ///
    /// `chain_ws` is the parachain WebSocket endpoint the provider connects to.
    /// `seed` is the key derivation path (e.g. `"//Alice"`).
    pub async fn spawn(chain_ws: &str, seed: &str) -> Result<Self> {
        let provider_bin = get_provider_binary_path();
        let bind_addr = provider_bind_addr();

        log::info!("Starting provider: {provider_bin}");
        log::info!("SEED={seed}");
        log::info!("CHAIN_RPC={chain_ws}");
        log::info!("BIND_ADDR={bind_addr}");

        let child = Command::new(&provider_bin)
            .env("SEED", seed)
            .env("CHAIN_RPC", chain_ws)
            .env("BIND_ADDR", &bind_addr)
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| anyhow!("Failed to start provider '{provider_bin}': {e}"))?;

        let mut provider = Self { child };
        provider.wait_healthy().await?;

        Ok(provider)
    }

    /// Poll the provider's /health endpoint until it responds.
    async fn wait_healthy(&mut self) -> Result<()> {
        let url = format!("{}/health", provider_url());
        let client = reqwest::Client::new();

        log::info!("Waiting for provider health at {url}");

        for attempt in
            1..=(PROVIDER_HEALTH_TIMEOUT_SECS * 1000 / PROVIDER_HEALTH_POLL_INTERVAL_MS) as u32
        {
            // Check the process hasn't exited
            if let Some(status) = self.child.try_wait()? {
                anyhow::bail!("Provider exited early with status: {status}");
            }

            match client.get(&url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    log::info!("Provider is healthy (attempt {attempt})");
                    return Ok(());
                }
                _ => {
                    tokio::time::sleep(Duration::from_millis(PROVIDER_HEALTH_POLL_INTERVAL_MS))
                        .await;
                }
            }
        }

        anyhow::bail!(
            "Provider did not become healthy within {PROVIDER_HEALTH_TIMEOUT_SECS} seconds",
        );
    }
}

impl Drop for ProviderProcess {
    fn drop(&mut self) {
        log::info!("Stopping provider process");
    }
}
