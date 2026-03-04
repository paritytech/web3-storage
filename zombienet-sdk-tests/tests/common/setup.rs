//! Shared setup helpers to reduce duplication across test files.

use super::config::*;
use super::network::{build_network_config, get_collator_ws, spawn_network};
use super::provider::ProviderProcess;
use anyhow::{anyhow, Result};
use storage_client::{ClientConfig, ProviderClient};
use zombienet_sdk::{LocalFileSystem, Network};

/// A running test environment: network + provider, kept alive via RAII.
///
/// All tests share the same setup: spawn a relay+parachain network, start a
/// storage-provider-node, and register Alice as the provider on-chain.
/// Use [`TestEnvironment::spawn`] at the top of each test instead of
/// duplicating the boilerplate.
pub struct TestEnvironment {
    pub chain_ws: String,
    pub provider_url: String,
    /// The registered provider client (Alice). Layer 0 tests use this directly;
    /// higher-layer tests can ignore it.
    pub alice_provider: ProviderClient,
    // Hold these to keep the processes alive for the test's lifetime.
    _network: Network<LocalFileSystem>,
    _provider: ProviderProcess,
}

impl TestEnvironment {
    /// Spin up a complete test environment:
    /// 1. Init env_logger
    /// 2. Build & spawn zombienet network (relay chain + parachain)
    /// 3. Start the storage provider node
    /// 4. Register Alice as provider on-chain
    pub async fn spawn() -> Result<Self> {
        let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
            .try_init();

        log::info!("Spawning network and provider...");
        let network_config = build_network_config()?;
        let network = spawn_network(network_config).await?;
        let chain_ws = get_collator_ws(&network, "collator-alice")?;
        let provider = ProviderProcess::spawn(&chain_ws, "//Alice").await?;
        let provider_url = provider_url();

        log::info!("Chain: {chain_ws}");
        log::info!("Provider: {provider_url}");

        log::info!("Registering provider on-chain...");
        let alice_provider = register_alice_provider(&chain_ws, &provider_url).await?;

        Ok(Self {
            chain_ws,
            provider_url,
            alice_provider,
            _network: network,
            _provider: provider,
        })
    }
}

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
        .register(provider_multiaddr(), alice_public_key, PROVIDER_STAKE)
        .await
        .map_err(|e| anyhow!("register_provider failed: {e}"))?;

    log::info!("Provider registered (Alice)");
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
                .map_err(|e| anyhow!("Invalid hex at offset {i}: {e}"))
        })
        .collect()
}
