//! Demo setup: register provider, create bucket, establish storage agreement.
//!
//! Usage: cargo run --release -p storage-client --bin demo_setup -- <chain_ws_url> <provider_url>

use storage_client::{AdminClient, ClientConfig, ProviderClient};

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

    // Use Alice as both admin and provider for demo simplicity
    // In production these would be different accounts
    let alice = "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY";

    // ═══════════════════════════════════════════════════════════════════════════
    // Step 1: Register Provider
    // ═══════════════════════════════════════════════════════════════════════════
    println!("Step 1: Registering provider...");

    let mut provider_client = ProviderClient::new(config.clone(), alice.to_string())?;
    provider_client.connect().await?;
    provider_client.set_dev_signer("alice")?;

    // MinProviderStake is 1000 tokens (1000 * 1e12 = 1e15)
    let stake = 1_000_000_000_000_000u128; // 1000 tokens

    match provider_client
        .register(
            format!("/ip4/127.0.0.1/tcp/3000"), // multiaddr
            vec![0u8; 32],                       // mock public key
            stake,
        )
        .await
    {
        Ok(_) => println!("  Provider registered successfully"),
        Err(e) => println!("  Provider registration: {} (may already be registered)", e),
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Step 2: Create Bucket
    // ═══════════════════════════════════════════════════════════════════════════
    println!("\nStep 2: Creating bucket...");

    let mut admin_client = AdminClient::new(config.clone(), alice.to_string())?;
    admin_client.connect().await?;
    admin_client.set_dev_signer("alice")?;

    let bucket_id = match admin_client.create_bucket(1).await {
        Ok(id) => {
            println!("  Bucket created with ID: {}", id);
            id
        }
        Err(e) => {
            println!("  Bucket creation failed: {}", e);
            println!("  Using bucket ID 1 (assuming it exists)");
            1
        }
    };

    // ═══════════════════════════════════════════════════════════════════════════
    // Step 3: Request Storage Agreement
    // ═══════════════════════════════════════════════════════════════════════════
    println!("\nStep 3: Requesting storage agreement...");

    match admin_client
        .request_agreement(
            bucket_id,
            alice.to_string(),        // provider (self for demo)
            1024 * 1024 * 1024,       // 1 GB capacity
            100_000,                  // ~1 week at 6 sec blocks
            100_000_000_000,          // 0.1 token payment
            None,                     // primary provider (not replica)
        )
        .await
    {
        Ok(_) => println!("  Agreement requested successfully"),
        Err(e) => println!("  Agreement request: {} (may already exist)", e),
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Step 4: Accept Agreement (provider side)
    // ═══════════════════════════════════════════════════════════════════════════
    println!("\nStep 4: Provider accepting agreement...");

    match provider_client.accept_agreement(bucket_id).await {
        Ok(_) => println!("  Agreement accepted successfully"),
        Err(e) => println!("  Agreement acceptance: {} (may already be accepted)", e),
    }

    println!("\n=== Setup Complete ===");
    println!();
    println!("You can now upload data:");
    println!("  just demo-upload");
    println!();
    println!("Bucket ID: {}", bucket_id);

    Ok(())
}
