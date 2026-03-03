//! Shared setup helpers to reduce duplication across test files.

use super::config::*;
use anyhow::{anyhow, Result};
use storage_client::{ClientConfig, ProviderClient};

/// Build a [`ClientConfig`] pointing at the given endpoints.
pub fn client_config(chain_ws: &str, provider_url: &str) -> ClientConfig {
    ClientConfig {
        chain_ws_url: chain_ws.to_string(),
        provider_urls: vec![provider_url.to_string()],
        timeout_secs: CLIENT_TIMEOUT_SECS,
        enable_retries: true,
    }
}

/// Create a [`ProviderClient`] for Alice, connect it, and register the provider on-chain.
pub async fn register_alice_provider(chain_ws: &str, provider_url: &str) -> Result<ProviderClient> {
    let config = client_config(chain_ws, provider_url);

    let mut client = ProviderClient::new(config, ALICE_SS58.to_string())
        .map_err(|e| anyhow!("ProviderClient::new failed: {e}"))?;
    client
        .connect()
        .await
        .map_err(|e| anyhow!("ProviderClient connect failed: {e}"))?;
    client
        .set_dev_signer("alice")
        .map_err(|e| anyhow!("ProviderClient set_dev_signer failed: {e}"))?;

    let alice_public_key = hex_to_bytes(ALICE_PUBLIC_KEY_HEX)?;
    client
        .register(
            PROVIDER_MULTIADDR.to_string(),
            alice_public_key,
            PROVIDER_STAKE,
        )
        .await
        .map_err(|e| anyhow!("register_provider failed: {e}"))?;

    log::info!("  Provider registered (Alice)");
    Ok(client)
}

/// Decode a hex string (with optional 0x prefix) into bytes.
pub fn hex_to_bytes(hex: &str) -> Result<Vec<u8>> {
    let hex = hex.strip_prefix("0x").unwrap_or(hex);
    if hex.len() % 2 != 0 {
        anyhow::bail!("Invalid hex length: {}", hex.len());
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&hex[i..i + 2], 16)
                .map_err(|e| anyhow!("Invalid hex at offset {}: {}", i, e))
        })
        .collect()
}
