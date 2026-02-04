//! Simple demo to upload test data to a provider.
//!
//! Usage: cargo run --release -p storage-client --bin demo_upload -- <provider_url> <bucket_id> [data]

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

    // Get data from third argument or use default
    let data: Vec<u8> = args.get(3)
        .map(|s| s.clone().into_bytes())
        .unwrap_or_else(|| b"Hello, Web3 Storage!".to_vec());

    println!("Provider:  {}", provider_url);
    println!("Bucket ID: {}", bucket_id);
    println!("Uploading {} bytes...", data.len());
    println!("Data: {:?}", String::from_utf8_lossy(&data));

    // Compute hash
    let hash: H256 = blake2_256(&data);
    let hash_hex = format!("0x{}", hex::encode(hash.as_bytes()));
    println!("Hash: {}", hash_hex);

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

    // Verify we can read it back
    println!("\nVerifying data...");
    let resp = client
        .get(format!("{}/node?hash={}", provider_url, hash_hex))
        .send()
        .await?;

    if resp.status().is_success() {
        #[derive(serde::Deserialize)]
        struct DownloadResponse {
            hash: String,
            data: String,
            children: Option<Vec<String>>,
        }

        let download_resp: DownloadResponse = resp.json().await?;
        let downloaded_data = BASE64.decode(&download_resp.data)?;

        println!("Data verified successfully!");
        println!("Downloaded: {:?}", String::from_utf8_lossy(&downloaded_data));

        if downloaded_data == data {
            println!("Data integrity check: PASSED");
        } else {
            println!("Data integrity check: FAILED");
        }
    } else {
        eprintln!("Verification failed!");
    }

    println!("\nDemo complete!");
    Ok(())
}
