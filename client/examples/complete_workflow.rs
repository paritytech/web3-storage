// SPDX-License-Identifier: Apache-2.0

//! Complete workflow example demonstrating all client types.
//!
//!   1. Start the chain:        just start-chain
//!   2. Start the provider:     just start-provider          (defaults to the //Alice key)
//!   3. Register the provider:  cargo run --example register_provider
//!
//! With those running, this example exercises only the user-facing flow against
//! the live chain + provider node:
//!   1. negotiate storage terms with the provider node (HTTP)
//!   2. establish a storage agreement on-chain (creates a bucket)
//!   3. upload data to the provider
//!   4. download it back and verify integrity
//!
//! Usage: cargo run --example complete_workflow [chain_ws] [provider_url] [user_seed]
//!
//! Arguments:
//!   chain_ws      - WebSocket URL for parachain      (default: ws://127.0.0.1:2222)
//!   provider_url  - HTTP URL for the provider node   (default: http://127.0.0.1:3333)
//!   user_seed     - Seed of the paying user          (default: //Bob)

use sp_core::crypto::Ss58Codec;
use sp_runtime::AccountId32;
use std::env;
use storage_client::{
    AdminClient, ChunkingStrategy, ClientConfig, NegotiateRequest, ProviderClient, SignedTerms,
    StorageUserClient,
};
use storage_subxt::subxt_signer::{sr25519::Keypair, SecretUri};

const DEFAULT_CHAIN_WS: &str = "ws://127.0.0.1:2222";
const DEFAULT_PROVIDER_URL: &str = "http://127.0.0.1:3333";
const DEFAULT_USER_SEED: &str = "//Bob";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let args: Vec<String> = env::args().collect();
    let chain_ws = args.get(1).map(String::as_str).unwrap_or(DEFAULT_CHAIN_WS);
    let provider_url = args
        .get(2)
        .map(String::as_str)
        .unwrap_or(DEFAULT_PROVIDER_URL);
    let user_seed = args.get(3).map(String::as_str).unwrap_or(DEFAULT_USER_SEED);

    let user_keypair = Keypair::from_uri(&user_seed.parse::<SecretUri>()?)?;
    let user_account = AccountId32::from(user_keypair.public_key().0);
    let user_ss58 = user_account.to_ss58check();

    let provider_ss58 = ProviderClient::fetch_provider_id(provider_url)
        .await?
        .to_ss58check();

    println!("=== Complete Storage Workflow ===");
    println!("Chain WebSocket: {chain_ws}");
    println!("Provider URL:    {provider_url}");
    println!("Provider (SS58): {provider_ss58}");
    println!("User (SS58):     {user_ss58}");
    println!();

    let chain_config = ClientConfig {
        chain_ws_url: chain_ws.to_string(),
        ..Default::default()
    };

    // 1. Negotiate terms with the running provider node.
    println!("Negotiating storage terms with provider...");
    let signed: SignedTerms = ProviderClient::negotiate_terms(
        provider_url,
        &NegotiateRequest {
            owner: user_account.clone(),
            max_bytes: 1_000_000,
            duration: 100,
            price_per_byte: 1,
            replica_params: None,
            bucket_id: None,
        },
    )
    .await?;
    assert!(signed.terms.nonce > 0, "provider should allocate a nonce");
    println!("  Terms signed (nonce={})", signed.terms.nonce);

    // 2. Redeem the signed terms on-chain to open a bucket + primary agreement.
    println!("Establishing storage agreement on-chain...");
    let mut admin = AdminClient::new(chain_config.clone(), user_ss58.clone())?;
    admin.connect().await?;
    admin.set_signer(user_keypair)?;
    let bucket_id = admin
        .establish_storage_agreement(provider_ss58, signed.terms, signed.signature)
        .await?;
    println!("  Bucket #{bucket_id} created");

    // 3. Upload + download against the provider node, verifying integrity.
    println!("Uploading and downloading data...");
    let user_config = ClientConfig {
        chain_ws_url: chain_ws.to_string(),
        provider_urls: vec![provider_url.to_string()],
        ..Default::default()
    };
    let user = StorageUserClient::new(user_config)?;
    let data = b"hello e2e".to_vec();
    let data_root = user
        .upload(bucket_id, &data, ChunkingStrategy::default())
        .await?;
    let downloaded = user.download(&data_root, 0, data.len() as u64).await?;
    assert_eq!(downloaded, data, "downloaded bytes must match upload");
    println!("  Uploaded {} bytes and verified download", data.len());

    println!();
    println!("=== Done ===");
    Ok(())
}
