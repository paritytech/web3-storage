// SPDX-License-Identifier: Apache-2.0

//! Provider registration and settings update.
//!
//! Mirrors the `registerProvider` + `updateProviderSettings` steps from
//! `examples/papi/full-flow.js`: registers the provider if not already present,
//! then sets price_per_byte=1 and accepting_primary=true.
//!
//! Usage: cargo run --example register_provider [chain_ws] [provider_url] [multiaddr] [keyfile]
//!
//! Arguments:
//!   chain_ws      - WebSocket URL for parachain   (default: ws://127.0.0.1:2222)
//!   provider_url  - HTTP URL for provider node    (default: http://127.0.0.1:3333)
//!   multiaddr     - Provider multiaddr            (default: /ip4/127.0.0.1/tcp/3333)
//!   keyfile       - Path to file containing seed  (default: dev seed //Alice)

use sp_core::crypto::Ss58Codec;
use std::env;
use storage_client::{ClientConfig, ProviderClient};
use storage_subxt::subxt_signer::{sr25519::Keypair, SecretUri};
use storage_subxt::storage_paseo_runtime::api::runtime_types::pallet_storage_provider::pallet::ProviderSettings;

const DEFAULT_CHAIN_WS: &str = "ws://127.0.0.1:2222";
const DEFAULT_PROVIDER_URL: &str = "http://127.0.0.1:3333";
const DEFAULT_PROVIDER_MULTIADDR: &str = "/ip4/127.0.0.1/tcp/3333";
const DEFAULT_KEYRING: &str = "//Alice";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let args: Vec<String> = env::args().collect();
    let chain_ws = args.get(1).map(String::as_str).unwrap_or(DEFAULT_CHAIN_WS);
    let provider_url = args
        .get(2)
        .map(String::as_str)
        .unwrap_or(DEFAULT_PROVIDER_URL);
    let multiaddr = args
        .get(3)
        .cloned()
        .unwrap_or_else(|| DEFAULT_PROVIDER_MULTIADDR.to_string());

    // Load keypair from keyfile seed (e.g. "//Alice") or fall back to //Alice.
    // The keyfile format matches start-provider: a single seed phrase per line.
    let keypair = if let Some(keyfile) = args.get(4) {
        let seed = std::fs::read_to_string(keyfile)
            .map_err(|e| format!("Failed to read keyfile {keyfile}: {e}"))?;
        let uri: SecretUri = seed.trim().parse()?;
        Keypair::from_uri(&uri)?
    } else {
        Keypair::from_uri(&DEFAULT_KEYRING.parse::<SecretUri>()?)?
    };

    // Derive SS58 address from the keypair for display and ProviderClient identity.
    let public_key_bytes = keypair.public_key().0;
    let account = sp_runtime::AccountId32::from(public_key_bytes);
    let ss58_address = account.to_ss58check();

    println!("=== Provider Registration ===");
    println!("Chain WebSocket: {chain_ws}");
    println!("Provider URL:    {provider_url}");
    println!("Multiaddr:       {multiaddr}");
    println!("Account (SS58):  {ss58_address}");
    println!();

    let config = ClientConfig {
        chain_ws_url: chain_ws.to_string(),
        ..Default::default()
    };

    let mut provider_client = ProviderClient::new(config, ss58_address.clone())?;
    provider_client.connect().await?;
    provider_client.set_signer(keypair.clone())?;

    // Step 1: Register (idempotent — skip if already registered).
    const STAKE: u128 = 1_000_000_000_000_000; // 1000 tokens (MinProviderStake, 12 decimals)

    let provider_info = provider_client.get_provider_info(&account).await?;
    if let Some(info) = provider_info {
        println!("Provider already existed");
        println!("{info:?}");
        return Ok(());
    }

    println!("Registering provider...");
    match provider_client
        .register(multiaddr, public_key_bytes.to_vec(), STAKE)
        .await
    {
        Ok(()) => println!("  Provider registered"),
        Err(e) => return Err(e.into()),
    }

    // Step 2: Update settings.
    println!("Updating provider settings...");
    provider_client
        .update_settings(ProviderSettings {
            price_per_byte: 1,
            min_duration: 10,
            max_duration: 100_000,
            accepting_primary: true,
            replica_sync_price: None,
            accepting_extensions: true,
            max_capacity: 0, // 0 = unlimited
        })
        .await?;
    println!("  Settings updated");

    println!();
    println!("=== Done ===");
    Ok(())
}
