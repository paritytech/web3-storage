//! Demo to submit an on-chain checkpoint for a bucket.
//!
//! Fetches a checkpoint-compatible signature from the provider, then submits
//! the `checkpoint` extrinsic on-chain. This creates a snapshot that enables
//! `challenge_checkpoint`.
//!
//! Usage: cargo run --release -p storage-client --bin demo_checkpoint -- <chain_ws_url> <bucket_id> <provider_url> <provider_account>

use sp_core::H256;
use storage_client::{AdminClient, ClientConfig, StorageUserClient};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();

    let chain_ws_url = args
        .get(1)
        .map(|s| s.as_str())
        .unwrap_or("ws://127.0.0.1:9944");

    let bucket_id: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1);

    let provider_url = args
        .get(3)
        .map(|s| s.as_str())
        .unwrap_or("http://127.0.0.1:3000");

    let provider_account = args
        .get(4)
        .cloned()
        .unwrap_or_else(|| "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY".to_string());

    println!("=== Submit On-Chain Checkpoint ===\n");
    println!("Chain:           {}", chain_ws_url);
    println!("Bucket ID:       {}", bucket_id);
    println!("Provider URL:    {}", provider_url);
    println!("Provider:        {}", provider_account);

    // Fetch checkpoint-compatible signature from provider
    println!("\nFetching checkpoint signature from provider...");
    let config = ClientConfig {
        chain_ws_url: chain_ws_url.to_string(),
        provider_urls: vec![provider_url.to_string()],
        timeout_secs: 30,
        enable_retries: true,
    };
    let user_client = StorageUserClient::new(config.clone())?;
    let checkpoint_sig = user_client.get_checkpoint_signature(bucket_id).await?;

    println!("MMR Root:        {}", checkpoint_sig.mmr_root);
    println!("Start Seq:       {}", checkpoint_sig.start_seq);
    println!("Leaf Count:      {}", checkpoint_sig.leaf_count);

    // Parse the signature and mmr_root
    let mmr_root_bytes = hex::decode(
        checkpoint_sig
            .mmr_root
            .strip_prefix("0x")
            .unwrap_or(&checkpoint_sig.mmr_root),
    )?;
    let mmr_root = H256::from_slice(&mmr_root_bytes);

    let signature_bytes = hex::decode(
        checkpoint_sig
            .provider_signature
            .strip_prefix("0x")
            .unwrap_or(&checkpoint_sig.provider_signature),
    )?;

    // Submit checkpoint on-chain (as Bob, the bucket admin)
    println!("\nSubmitting checkpoint on-chain...");
    let bob = "5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty".to_string();
    let mut admin = AdminClient::new(config, bob)?;
    admin.connect().await?;
    admin.set_dev_signer("bob")?;

    admin
        .submit_checkpoint(
            bucket_id,
            mmr_root,
            checkpoint_sig.start_seq,
            checkpoint_sig.leaf_count,
            vec![(provider_account.clone(), signature_bytes)],
        )
        .await?;

    println!("Checkpoint submitted successfully!");
    println!("\nThe bucket now has an on-chain snapshot. challenge_checkpoint can be used.");

    Ok(())
}
