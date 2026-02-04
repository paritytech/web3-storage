//! Demo setup: register provider, create bucket, establish storage agreement.
//!
//! Usage: cargo run --release -p storage-client --bin demo_setup -- <chain_ws_url> <provider_url>

use sp_core::{Pair, sr25519};
use sp_runtime::AccountId32;
use std::str::FromStr;
use storage_client::{AdminClient, ClientConfig, ProviderClient};
use storage_client::substrate::{storage, SubstrateClient};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();

    let chain_ws_url = args
        .get(1)
        .map(|s| s.as_str())
        .unwrap_or("ws://127.0.0.1:9944");

    let provider_url = args
        .get(2)
        .map(|s| s.as_str())
        .unwrap_or("http://127.0.0.1:3000");

    println!("=== Demo Setup ===");
    println!("Chain:    {}", chain_ws_url);
    println!("Provider: {}", provider_url);
    println!();

    let config = ClientConfig {
        chain_ws_url: chain_ws_url.to_string(),
        provider_urls: vec![provider_url.to_string()],
        timeout_secs: 30,
        enable_retries: true,
    };

    // Alice = provider, Bob = client/admin
    let alice = "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY";
    let bob = "5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty";
    let alice_account = AccountId32::from_str(alice)?;

    // Connect to chain for queries
    let chain = SubstrateClient::connect(chain_ws_url).await?;

    // ═══════════════════════════════════════════════════════════════════════════
    // Step 1: Check/Register Provider
    // ═══════════════════════════════════════════════════════════════════════════
    println!("Step 1: Checking provider registration...");

    let provider_exists = chain
        .api()
        .storage()
        .at_latest()
        .await?
        .fetch(&storage::provider_info(&alice_account))
        .await?
        .is_some();

    if provider_exists {
        println!("  Provider already registered");
    } else {
        println!("  Registering provider...");

        let mut provider_client = ProviderClient::new(config.clone(), alice.to_string())?;
        provider_client.connect().await?;
        provider_client.set_dev_signer("alice")?;

        // Get Alice's actual sr25519 public key for signature verification
        let alice_keypair = sr25519::Pair::from_string("//Alice", None)
            .expect("Failed to create Alice keypair");
        let alice_public_key = alice_keypair.public().0.to_vec();

        // MinProviderStake is 1000 tokens (1000 * 1e12 = 1e15)
        let stake = 1_000_000_000_000_000u128; // 1000 tokens

        match provider_client
            .register(
                format!("/ip4/127.0.0.1/tcp/3000"), // multiaddr
                alice_public_key,                    // Alice's actual sr25519 public key
                stake,
            )
            .await
        {
            Ok(_) => println!("  Provider registered successfully"),
            Err(e) => println!("  Provider registration failed: {}", e),
        }
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Step 2: Check/Create Bucket
    // ═══════════════════════════════════════════════════════════════════════════
    println!("\nStep 2: Checking bucket...");

    let bucket_id: u64 = 1;
    let bucket_exists = chain
        .api()
        .storage()
        .at_latest()
        .await?
        .fetch(&storage::bucket_info(bucket_id))
        .await?
        .is_some();

    if bucket_exists {
        println!("  Bucket {} already exists", bucket_id);
    } else {
        println!("  Creating bucket...");

        let mut admin_client = AdminClient::new(config.clone(), bob.to_string())?;
        admin_client.connect().await?;
        admin_client.set_dev_signer("bob")?;

        match admin_client.create_bucket(1).await {
            Ok(id) => println!("  Bucket created with ID: {}", id),
            Err(e) => println!("  Bucket creation failed: {}", e),
        }
        match admin_client.create_bucket(1).await {
            Ok(id) => println!("  Bucket created with ID: {}", id),
            Err(e) => println!("  Bucket creation failed: {}", e),
        }
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Step 3: Check/Create Agreement
    // ═══════════════════════════════════════════════════════════════════════════
    println!("\nStep 3: Checking storage agreement...");

    let agreement_exists = chain
        .api()
        .storage()
        .at_latest()
        .await?
        .fetch(&storage::agreement_info(bucket_id, &alice_account))
        .await?
        .is_some();

    if agreement_exists {
        println!("  Agreement already exists for bucket {}", bucket_id);
    } else {
        println!("  Requesting storage agreement...");

        let mut admin_client = AdminClient::new(config.clone(), bob.to_string())?;
        admin_client.connect().await?;
        admin_client.set_dev_signer("bob")?;

        match admin_client
            .request_agreement(
                bucket_id,
                alice.to_string(),        // provider is Alice
                1024 * 1024 * 1024,       // 1 GB capacity
                100_000,                  // ~1 week at 6 sec blocks
                100_000_000_000,          // 0.1 token payment
                None,                     // primary provider (not replica)
            )
            .await
        {
            Ok(_) => {
                println!("  Agreement requested successfully");

                // Step 4: Accept Agreement (provider side - Alice)
                println!("\nStep 4: Provider (Alice) accepting agreement...");

                let mut provider_client = ProviderClient::new(config.clone(), alice.to_string())?;
                provider_client.connect().await?;
                provider_client.set_dev_signer("alice")?;

                match provider_client.accept_agreement(bucket_id).await {
                    Ok(_) => println!("  Agreement accepted successfully"),
                    Err(e) => println!("  Agreement acceptance failed: {}", e),
                }
            }
            Err(e) => println!("  Agreement request failed: {}", e),
        }
    }

    println!("\n=== Setup Complete ===");
    println!();
    println!("You can now upload data:");
    println!("  just demo-upload");
    println!();
    println!("Bucket ID: {}", bucket_id);

    Ok(())
}
