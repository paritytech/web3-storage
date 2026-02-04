//! Simple demo to upload test data to a provider.
//!
//! Usage: cargo run --release -p storage-client --bin demo_upload -- <provider_url> <bucket_id> <chain_ws_url> [data]

use storage_client::{ChunkingStrategy, ClientConfig, StorageUserClient};

/// Output struct containing all upload results.
#[derive(serde::Serialize)]
struct UploadResult {
    // Upload info
    provider_url: String,
    chain_ws_url: String,
    bucket_id: u64,
    data_size: usize,
    content_hash: String,

    // Commit info
    mmr_root: String,
    start_seq: u64,
    leaf_indices: Vec<u64>,
    provider_signature: String,

    // Verification
    verified: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();

    // Get provider URL from first argument
    let provider_url = args
        .get(1)
        .map(|s| s.as_str())
        .unwrap_or("http://127.0.0.1:3000");

    // Get bucket ID from second argument
    let bucket_id: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1);

    // Get chain WebSocket URL from third argument
    let chain_ws_url = args
        .get(3)
        .map(|s| s.as_str())
        .unwrap_or("ws://127.0.0.1:9944");

    // Get data from fourth argument or use default
    let data: Vec<u8> = args
        .get(4)
        .map(|s| s.clone().into_bytes())
        .unwrap_or_else(|| b"Hello, Web3 Storage!".to_vec());

    println!("Provider:  {}", provider_url);
    println!("Chain:     {}", chain_ws_url);
    println!("Bucket ID: {}", bucket_id);
    println!("Uploading {} bytes...", data.len());
    println!("Data: {:?}", String::from_utf8_lossy(&data));

    // Create StorageUserClient
    let config = ClientConfig {
        chain_ws_url: chain_ws_url.to_string(),
        provider_urls: vec![provider_url.to_string()],
        timeout_secs: 30,
        enable_retries: true,
    };
    let client = StorageUserClient::new(config)?;

    // Upload via StorageUserClient
    let data_root = client.upload(bucket_id, &data, ChunkingStrategy::default()).await?;
    let hash_hex = format!("0x{}", hex::encode(data_root.as_bytes()));
    println!("Data Root: {}", hash_hex);

    // Commit to MMR
    println!("\nCommitting to MMR...");
    let commit_resp = client.commit(bucket_id, vec![data_root]).await?;
    println!("MMR Root: {}", commit_resp.mmr_root);
    println!("Start Seq: {}", commit_resp.start_seq);
    println!("Leaf Indices: {:?}", commit_resp.leaf_indices);

    // Verify we can read it back
    println!("\nVerifying data using StorageUserClient...");
    let verified = match client.download(&data_root, 0, data.len() as u64).await {
        Ok(downloaded_data) => {
            println!("Data verified successfully!");
            println!(
                "Downloaded: {:?}",
                String::from_utf8_lossy(&downloaded_data)
            );

            if downloaded_data == data {
                println!("Data integrity check: PASSED");
                true
            } else {
                println!("Data integrity check: FAILED (content mismatch)");
                false
            }
        }
        Err(e) => {
            eprintln!("Verification failed: {}", e);
            false
        }
    };

    let result = UploadResult {
        provider_url: provider_url.to_string(),
        chain_ws_url: chain_ws_url.to_string(),
        bucket_id,
        data_size: data.len(),
        content_hash: hash_hex,
        mmr_root: commit_resp.mmr_root,
        start_seq: commit_resp.start_seq,
        leaf_indices: commit_resp.leaf_indices,
        provider_signature: commit_resp.provider_signature,
        verified,
    };

    // Output JSON result
    println!("\n{}", serde_json::to_string_pretty(&result)?);

    Ok(())
}
