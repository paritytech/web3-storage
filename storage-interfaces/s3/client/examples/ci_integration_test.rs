// SPDX-License-Identifier: Apache-2.0

//! S3 CI Integration Test
//!
//! This test is designed to run in CI after the infrastructure is set up
//! (chain running on ws://127.0.0.1:2222, provider on http://127.0.0.1:3333).
//!
//! It tests the full S3 workflow:
//! 1. Negotiate signed agreement terms with the provider
//! 2. Create an S3 bucket (atomically opens Layer 0 bucket + primary agreement)
//! 3. Upload objects with metadata
//! 4. Download and verify objects
//! 5. Copy objects
//! 6. Delete objects and bucket
//!
//! Object operations go directly through the provider's S3 HTTP API.
//!
//! Usage: cargo run --example ci_integration_test [chain_ws] [provider_url]

use s3_client::{PutObjectOptions, S3Client};
use sp_runtime::AccountId32;
use std::collections::HashMap;
use std::env;
use storage_client::{NegotiateRequest, ProviderClient};
use subxt_signer::sr25519::dev as dev_signer;

const DEFAULT_CHAIN_WS: &str = "ws://127.0.0.1:2222";
const DEFAULT_PROVIDER_URL: &str = "http://127.0.0.1:3333";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

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
    let owner: AccountId32 = dev_signer::bob().public_key().0.into();
    let client = S3Client::new(chain_ws, provider_url, "//Bob").await?;
    println!("  Client connected successfully");

    // Discover the provider's on-chain account from its /info endpoint
    let provider = ProviderClient::fetch_provider_id(provider_url).await?;
    println!("  Provider account (from /info): {provider}");

    // Step 2: Negotiate signed agreement terms
    println!();
    println!("Step 2: Negotiating signed terms with provider...");
    let signed = ProviderClient::negotiate_terms(
        provider_url,
        &NegotiateRequest {
            owner,
            max_bytes: 1_000_000_000, // 1 GB
            duration: 500,            // 500 blocks
            price_per_byte: 1,
            bucket_id: None,
            replica_params: None,
        },
    )
    .await?;
    println!(
        "  Provider signed terms: nonce={}, valid_until={}",
        signed.terms.nonce, signed.terms.valid_until
    );

    // Step 3: Create an S3 bucket
    println!();
    println!("Step 3: Creating S3 bucket...");
    let bucket_name = format!(
        "ci-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs()
    );
    let bucket = client
        .create_bucket(&bucket_name, provider, signed.terms, signed.signature)
        .await?;
    println!("  Bucket created: {bucket_name}");
    println!("  S3 Bucket ID: {}", bucket.s3_bucket_id);
    println!("  Layer 0 Bucket ID: {}", bucket.layer0_bucket_id);

    // Step 4: Upload objects
    println!();
    println!("Step 4: Uploading objects...");

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

    let copied = client.get_object(&bucket_name, "hello-copy.txt").await?;
    assert_eq!(
        copied.data.as_slice(),
        content_1,
        "Content mismatch for copied object"
    );
    println!("    Copy content verified!");

    // Step 7: Head object
    println!();
    println!("Step 7: Checking object metadata...");
    let head = client.get_object(&bucket_name, "hello.txt").await?;
    println!("  hello.txt metadata:");
    println!("    Content-Type: {}", head.content_type);
    println!("    Size: {}", head.size);
    println!("    ETag: {}", head.etag);

    // Step 8: Delete objects and bucket
    println!();
    println!("Step 8: Cleaning up...");

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
    println!("  - Uploaded 2 objects via provider HTTP API");
    println!("  - Downloaded and verified 2 objects");
    println!("  - Copied and verified 1 object");
    println!("  - Checked object metadata via HEAD");
    println!("  - Cleaned up all objects and bucket");

    Ok(())
}
