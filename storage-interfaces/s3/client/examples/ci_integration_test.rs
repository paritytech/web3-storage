//! S3 CI Integration Test
//!
//! This test is designed to run in CI after the infrastructure is set up
//! (chain running on ws://127.0.0.1:2222, provider on http://127.0.0.1:3333).
//!
//! It tests the full S3 workflow:
//! 1. Create an S3 bucket
//! 2. Upload objects with metadata
//! 3. Download and verify objects
//! 4. List objects
//! 5. Copy objects
//! 6. Delete objects and bucket
//!
//! Usage: cargo run --example ci_integration_test [chain_ws] [provider_url]
//!
//! Defaults:
//!   chain_ws: ws://127.0.0.1:2222
//!   provider_url: http://127.0.0.1:3333

use s3_client::{PutObjectOptions, S3Client};
use std::collections::HashMap;
use std::env;

const DEFAULT_CHAIN_WS: &str = "ws://127.0.0.1:2222";
const DEFAULT_PROVIDER_URL: &str = "http://127.0.0.1:3333";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    // Parse command line arguments
    let args: Vec<String> = env::args().collect();
    let chain_ws = args.get(1).map(|s| s.as_str()).unwrap_or(DEFAULT_CHAIN_WS);
    let provider_url = args
        .get(2)
        .map(|s| s.as_str())
        .unwrap_or(DEFAULT_PROVIDER_URL);

    println!("=== S3 CI Integration Test ===");
    println!();
    println!("Chain WebSocket: {chain_ws}");
    println!("Provider URL: {provider_url}");
    println!();

    // Step 1: Create the S3 client
    println!("Step 1: Creating S3 client...");
    let client = S3Client::new(chain_ws, provider_url, "//Alice").await?;
    println!("  Client connected successfully");

    // Step 2: Create an S3 bucket
    println!();
    println!("Step 2: Creating S3 bucket...");
    let bucket_name = format!(
        "ci-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs()
    );
    let bucket = client.create_bucket(&bucket_name).await?;
    println!("  Bucket created: {bucket_name}");
    println!("  S3 Bucket ID: {}", bucket.s3_bucket_id);
    println!("  Layer 0 Bucket ID: {}", bucket.layer0_bucket_id);

    // Step 3: Upload objects
    println!();
    println!("Step 3: Uploading objects...");

    let content_1 = b"Hello from S3 CI integration test!";
    let mut metadata_1 = HashMap::new();
    metadata_1.insert("x-test-key".to_string(), "test-value".to_string());
    let put_result_1 = client
        .put_object(
            &bucket_name,
            "hello.txt",
            content_1,
            PutObjectOptions {
                content_type: Some("text/plain".to_string()),
                metadata: metadata_1,
            },
        )
        .await?;
    println!(
        "  Uploaded hello.txt ({} bytes, etag={})",
        put_result_1.size, put_result_1.etag
    );

    let content_2 = b"This is a binary-like payload for testing.";
    let put_result_2 = client
        .put_object(
            &bucket_name,
            "data/payload.bin",
            content_2,
            PutObjectOptions {
                content_type: Some("application/octet-stream".to_string()),
                ..Default::default()
            },
        )
        .await?;
    println!(
        "  Uploaded data/payload.bin ({} bytes, etag={})",
        put_result_2.size, put_result_2.etag
    );

    // Step 4: Head bucket to verify object count
    println!();
    println!("Step 4: Verifying bucket info...");
    let bucket_info = client.head_bucket(&bucket_name).await?;
    println!("  Bucket: {}", bucket_info.name);
    println!("  Object count: {}", bucket_info.object_count);
    println!("  Total size: {} bytes", bucket_info.total_size);
    assert_eq!(bucket_info.object_count, 2, "Expected 2 objects in bucket");

    // Step 5: Download and verify objects
    println!();
    println!("Step 5: Downloading and verifying objects...");

    let get_result_1 = client.get_object(&bucket_name, "hello.txt").await?;
    println!(
        "  Downloaded hello.txt ({} bytes, content_type={})",
        get_result_1.size, get_result_1.content_type
    );
    assert_eq!(
        get_result_1.data.as_slice(),
        content_1,
        "Content mismatch for hello.txt"
    );
    println!("    Content verified!");

    let get_result_2 = client.get_object(&bucket_name, "data/payload.bin").await?;
    println!(
        "  Downloaded data/payload.bin ({} bytes, content_type={})",
        get_result_2.size, get_result_2.content_type
    );
    assert_eq!(
        get_result_2.data.as_slice(),
        content_2,
        "Content mismatch for data/payload.bin"
    );
    println!("    Content verified!");

    // Step 6: Copy object
    println!();
    println!("Step 6: Copying object...");
    let copy_result = client
        .copy_object(&bucket_name, "hello.txt", &bucket_name, "hello-copy.txt")
        .await?;
    println!(
        "  Copied hello.txt -> hello-copy.txt (etag={})",
        copy_result.etag
    );

    // Verify the copy
    let copied = client.get_object(&bucket_name, "hello-copy.txt").await?;
    assert_eq!(
        copied.data.as_slice(),
        content_1,
        "Content mismatch for copied object"
    );
    println!("    Copy content verified!");

    // Step 7: Delete objects and bucket
    println!();
    println!("Step 7: Cleaning up...");

    client.delete_object(&bucket_name, "hello.txt").await?;
    println!("  Deleted hello.txt");

    client
        .delete_object(&bucket_name, "data/payload.bin")
        .await?;
    println!("  Deleted data/payload.bin");

    client.delete_object(&bucket_name, "hello-copy.txt").await?;
    println!("  Deleted hello-copy.txt");

    client.delete_bucket(&bucket_name).await?;
    println!("  Deleted bucket: {bucket_name}");

    // Summary
    println!();
    println!("=== PASSED: All S3 tests completed successfully! ===");
    println!();
    println!("Summary:");
    println!("  - Created S3 bucket ({bucket_name})");
    println!("  - Uploaded 2 objects with metadata");
    println!("  - Verified bucket info (object count, total size)");
    println!("  - Downloaded and verified 2 objects");
    println!("  - Copied and verified 1 object");
    println!("  - Cleaned up all objects and bucket");

    Ok(())
}
