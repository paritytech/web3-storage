//! Simple demo to upload test data to a provider.
//!
//! Usage: cargo run --release -p storage-client --bin demo_upload -- <provider_url> <bucket_id> <chain_ws_url> [data]

use sp_core::H256;
use storage_primitives::blake2_256;

#[derive(serde::Serialize)]
struct UploadNodeRequest {
    bucket_id: u64,
    hash: String,
    data: String,
    children: Option<Vec<String>>,
}

#[derive(serde::Deserialize)]
struct UploadNodeResponse {
    stored: bool,
}

#[derive(serde::Serialize)]
struct CommitRequest {
    bucket_id: u64,
    data_roots: Vec<String>,
}

#[derive(serde::Deserialize)]
struct CommitResponse {
    mmr_root: String,
    start_seq: u64,
    leaf_indices: Vec<u64>,
    provider_signature: String,
}

/// Output struct containing all upload results and challenge information.
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

    // Challenge info
    challenge: ChallengeInfo,

    // Verification
    verified: bool,
}

#[derive(serde::Serialize)]
struct ChallengeInfo {
    provider_account: String,
    bucket_id: u64,
    leaf_index: u64,
    chunk_count: usize,
    max_chunk_index: usize,
    command: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();

    // Get provider URL from first argument
    let provider_url = args.get(1)
        .map(|s| s.as_str())
        .unwrap_or("http://127.0.0.1:3000");

    // Get bucket ID from second argument
    let bucket_id: u64 = args.get(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);

    // Get chain WebSocket URL from third argument
    let chain_ws_url = args.get(3)
        .map(|s| s.as_str())
        .unwrap_or("ws://127.0.0.1:9944");

    // Get data from fourth argument or use default
    let data: Vec<u8> = args.get(4)
        .map(|s| s.clone().into_bytes())
        .unwrap_or_else(|| b"Hello, Web3 Storage!".to_vec());

    println!("Provider:  {}", provider_url);
    println!("Chain:     {}", chain_ws_url);
    println!("Bucket ID: {}", bucket_id);
    println!("Uploading {} bytes...", data.len());
    println!("Data: {:?}", String::from_utf8_lossy(&data));

    // Compute content hash (blake2_256)
    let content_hash: H256 = blake2_256(&data);
    let hash_hex = format!("0x{}", hex::encode(content_hash.as_bytes()));
    println!("Content Hash: {}", hash_hex);

    // Encode data as base64
    use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
    let data_b64 = BASE64.encode(&data);

    // Upload node
    let client = reqwest::Client::new();
    let upload_req = UploadNodeRequest {
        bucket_id: bucket_id,
        hash: hash_hex.clone(),
        data: data_b64,
        children: None,
    };

    let resp = client
        .put(format!("{}/node", provider_url))
        .json(&upload_req)
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await?;
        eprintln!("Upload failed: {} - {}", status, body);
        return Err(format!("Upload failed: {}", status).into());
    }

    let upload_resp: UploadNodeResponse = resp.json().await?;
    println!("Stored: {}", upload_resp.stored);

    // Commit to MMR
    println!("\nCommitting to MMR...");
    let commit_req = CommitRequest {
        bucket_id: bucket_id,
        data_roots: vec![hash_hex.clone()],
    };

    let resp = client
        .post(format!("{}/commit", provider_url))
        .json(&commit_req)
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await?;
        eprintln!("Commit failed: {} - {}", status, body);
        return Err(format!("Commit failed: {}", status).into());
    }

    let commit_resp: CommitResponse = resp.json().await?;
    println!("MMR Root: {}", commit_resp.mmr_root);
    println!("Start Seq: {}", commit_resp.start_seq);
    println!("Leaf Indices: {:?}", commit_resp.leaf_indices);

    // Verify we can read it back using the client library
    println!("\nVerifying data using StorageUserClient...");

    use storage_client::{StorageUserClient, ClientConfig};

    let config = ClientConfig {
        chain_ws_url: chain_ws_url.to_string(),
        provider_urls: vec![provider_url.to_string()],
        timeout_secs: 30,
        enable_retries: true,
    };

    let storage_client = StorageUserClient::new(config)?;

    let verified = match storage_client.read_node_verified(&content_hash).await {
        Ok(downloaded_data) => {
            println!("Data verified successfully!");
            println!("Downloaded: {:?}", String::from_utf8_lossy(&downloaded_data));

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

    // Build challenge information
    // Provider account (Alice is the default provider from demo-setup)
    let provider_account = "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY".to_string();

    // Calculate chunk count (256 KiB chunks)
    let chunk_size = 256 * 1024;
    let num_chunks = (data.len() + chunk_size - 1) / chunk_size;
    let leaf_index = *commit_resp.leaf_indices.first().unwrap_or(&0);

    let challenge_command = format!(
        "just demo-challenge CHAIN_WS=\"{}\" BUCKET_ID=\"{}\" PROVIDER=\"{}\" LEAF=\"{}\" CHUNK=\"0\" MMR_ROOT=\"{}\" START_SEQ=\"{}\" SIGNATURE=\"{}\"",
        chain_ws_url, bucket_id, provider_account, leaf_index,
        commit_resp.mmr_root, commit_resp.start_seq, commit_resp.provider_signature
    );

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
        challenge: ChallengeInfo {
            provider_account,
            bucket_id,
            leaf_index,
            chunk_count: num_chunks,
            max_chunk_index: num_chunks.saturating_sub(1),
            command: challenge_command,
        },
        verified,
    };

    // Output JSON result
    println!("\n{}", serde_json::to_string_pretty(&result)?);

    Ok(())
}
