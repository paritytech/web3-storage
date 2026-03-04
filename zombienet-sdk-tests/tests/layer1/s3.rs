//! Layer 1 - S3-Compatible Interface Integration Tests
//!
//! Tests the full S3 workflow:
//! 1. Spawn network + provider (via common helpers)
//! 2. Register provider on-chain
//! 3. Create an S3 bucket
//! 4. Upload objects with metadata
//! 5. Download and verify objects
//! 6. Copy objects
//! 7. Delete objects and bucket

use crate::common::setup::TestEnvironment;
use anyhow::{Context, Result};
use s3_client::{PutObjectOptions, S3Client};
use std::collections::HashMap;

#[tokio::test(flavor = "multi_thread")]
async fn s3_integration_test() -> Result<()> {
    log::info!("=== Layer 1: S3-Compatible Interface Integration Test ===");

    let env = TestEnvironment::spawn().await?;

    // Step 1: Create the S3 client
    log::info!("Step 1: Creating S3 client...");
    let client = S3Client::new(&env.chain_ws, &env.provider_url, "//Alice")
        .await
        .context("Failed to create S3Client")?;
    log::info!("  Client connected successfully");

    // Step 3: Create an S3 bucket
    log::info!("Step 3: Creating S3 bucket...");
    let bucket_name = format!(
        "zombie-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    );
    let bucket = client
        .create_bucket(&bucket_name)
        .await
        .context("Failed to create S3 bucket")?;
    log::info!("  Bucket created: {bucket_name}");
    log::info!("  S3 Bucket ID: {}", bucket.s3_bucket_id);
    log::info!("  Layer 0 Bucket ID: {}", bucket.layer0_bucket_id);

    // Step 4: Upload objects
    log::info!("Step 4: Uploading objects...");

    let content_1 = b"Hello from zombienet-sdk S3 integration test!";
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
        .await
        .context("Failed to upload hello.txt")?;
    log::info!(
        "  Uploaded hello.txt ({} bytes, etag={})",
        put_result_1.size,
        put_result_1.etag
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
        .await
        .context("Failed to upload data/payload.bin")?;
    log::info!(
        "  Uploaded data/payload.bin ({} bytes, etag={})",
        put_result_2.size,
        put_result_2.etag
    );

    // Step 5: Verify bucket info
    log::info!("Step 5: Verifying bucket info...");
    let bucket_info = client
        .head_bucket(&bucket_name)
        .await
        .context("Failed to head bucket")?;
    log::info!("  Object count: {}", bucket_info.object_count);
    log::info!("  Total size: {} bytes", bucket_info.total_size);
    assert_eq!(bucket_info.object_count, 2, "Expected 2 objects in bucket");

    // Step 6: Download and verify objects
    log::info!("Step 6: Downloading and verifying objects...");

    let get_result_1 = client
        .get_object(&bucket_name, "hello.txt")
        .await
        .context("Failed to download hello.txt")?;
    log::info!(
        "  Downloaded hello.txt ({} bytes, content_type={})",
        get_result_1.size,
        get_result_1.content_type
    );
    assert_eq!(
        get_result_1.data.as_slice(),
        content_1,
        "Content mismatch for hello.txt"
    );
    log::info!("    Content verified!");

    let get_result_2 = client
        .get_object(&bucket_name, "data/payload.bin")
        .await
        .context("Failed to download data/payload.bin")?;
    log::info!(
        "  Downloaded data/payload.bin ({} bytes, content_type={})",
        get_result_2.size,
        get_result_2.content_type
    );
    assert_eq!(
        get_result_2.data.as_slice(),
        content_2,
        "Content mismatch for data/payload.bin"
    );
    log::info!("    Content verified!");

    // Step 7: Copy object
    log::info!("Step 7: Copying object...");
    let copy_result = client
        .copy_object(&bucket_name, "hello.txt", &bucket_name, "hello-copy.txt")
        .await
        .context("Failed to copy hello.txt")?;
    log::info!(
        "  Copied hello.txt -> hello-copy.txt (etag={})",
        copy_result.etag
    );

    let copied = client
        .get_object(&bucket_name, "hello-copy.txt")
        .await
        .context("Failed to download hello-copy.txt")?;
    assert_eq!(
        copied.data.as_slice(),
        content_1,
        "Content mismatch for copied object"
    );
    log::info!("    Copy content verified!");

    // Step 8: Clean up
    log::info!("Step 8: Cleaning up...");
    client
        .delete_object(&bucket_name, "hello.txt")
        .await
        .context("Failed to delete hello.txt")?;
    log::info!("  Deleted hello.txt");

    client
        .delete_object(&bucket_name, "data/payload.bin")
        .await
        .context("Failed to delete data/payload.bin")?;
    log::info!("  Deleted data/payload.bin");

    client
        .delete_object(&bucket_name, "hello-copy.txt")
        .await
        .context("Failed to delete hello-copy.txt")?;
    log::info!("  Deleted hello-copy.txt");

    client
        .delete_bucket(&bucket_name)
        .await
        .context("Failed to delete bucket")?;
    log::info!("  Deleted bucket: {bucket_name}");

    // Summary
    log::info!("=== PASSED: S3-Compatible Interface Integration Test ===");
    log::info!("  - Created S3 bucket ({bucket_name})");
    log::info!("  - Uploaded 2 objects with metadata");
    log::info!("  - Verified bucket info");
    log::info!("  - Downloaded and verified 2 objects");
    log::info!("  - Copied and verified 1 object");
    log::info!("  - Cleaned up all objects and bucket");

    Ok(())
}
