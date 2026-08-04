// SPDX-License-Identifier: Apache-2.0

//! Provider registration and settings update.
//!
//! Mirrors the `registerProvider` + `updateProviderSettings` steps from
//! `examples/papi/full-flow.js`: registers the provider if not already present,
//! then sets price_per_byte=1 and accepting_primary=true.
//!
//! Usage: cargo run --example register_provider [chain_ws] [provider_url] [multiaddr] [keyfile] [scheme]
//!
//! Arguments:
//!   chain_ws      - WebSocket URL for parachain   (default: ws://127.0.0.1:2222)
//!   provider_url  - HTTP URL for provider node    (default: http://127.0.0.1:3333)
//!   multiaddr     - Provider multiaddr            (default: /ip4/127.0.0.1/tcp/3333)
//!   keyfile       - Path to file containing seed  (default: dev seed //Alice)
//!   scheme        - Signing-key scheme registered as public_key:
//!                   sr25519|ed25519|ecdsa|eth     (default: sr25519)
//!
//! Extrinsics are always submitted from the sr25519 account derived from the
//! seed; `scheme` only selects the signing key registered on-chain (what the
//! provider node signs commitments/terms with — its --key-scheme must match).

use sp_core::crypto::Ss58Codec;
use sp_core::Pair as _;
use std::env;
use storage_client::{ClientConfig, ProviderClient, ProviderSettings};
use subxt_signer::{sr25519::Keypair, SecretUri};

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

    // Load the seed from a keyfile (e.g. "//Alice") or fall back to //Alice.
    // The keyfile format matches start-provider: a single seed phrase per line.
    let seed = if let Some(keyfile) = args.get(4) {
        std::fs::read_to_string(keyfile)
            .map_err(|e| format!("Failed to read keyfile {keyfile}: {e}"))?
            .trim()
            .to_string()
    } else {
        DEFAULT_KEYRING.to_string()
    };
    let keypair = Keypair::from_uri(&seed.parse::<SecretUri>()?)?;

    // The registered public_key is the signing key of the chosen scheme,
    // derived from the same seed; the submission account stays sr25519.
    let scheme = args.get(5).map(String::as_str).unwrap_or("sr25519");
    let signing_key: Vec<u8> = match scheme {
        "sr25519" => sp_core::sr25519::Pair::from_string(&seed, None)?
            .public()
            .0
            .to_vec(),
        "ed25519" => sp_core::ed25519::Pair::from_string(&seed, None)?
            .public()
            .0
            .to_vec(),
        "ecdsa" => sp_core::ecdsa::Pair::from_string(&seed, None)?
            .public()
            .0
            .to_vec(),
        "eth" => sp_core::ecdsa::KeccakPair::from_string(&seed, None)?
            .public()
            .0
            .to_vec(),
        other => {
            return Err(format!("Unknown scheme '{other}' (sr25519|ed25519|ecdsa|eth)").into())
        }
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
    println!("Key scheme:      {scheme}");
    println!();

    let config = ClientConfig {
        chain_ws_url: chain_ws.to_string(),
        ..Default::default()
    };

    let mut provider_client = ProviderClient::new(config, keypair.clone().into())?;
    provider_client.connect().await?;

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
        .register(multiaddr, signing_key, STAKE)
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
