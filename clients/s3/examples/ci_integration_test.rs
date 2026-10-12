// SPDX-License-Identifier: Apache-2.0

//! S3 CI Integration Test
//!
//! This test is designed to run in CI after the infrastructure is set up
//! (chain running on ws://127.0.0.1:2222, provider on http://127.0.0.1:3333).
//!
//! It tests the full S3 workflow:
//! 1. Negotiate signed agreement terms with the provider
//! 2. Create an S3 bucket (a Layer 0 bucket with one primary agreement)
//! 3. Upload objects with metadata
//! 4. Download and verify objects
//! 5. List objects
//! 6. Read object metadata
//! 7. Delete objects
//!
//! Object operations go directly through the provider's S3 HTTP API.
//!
//! Usage: cargo run --example ci_integration_test [chain_ws] [provider_url]

use s3_client::{ListObjectsParams, PutObjectOptions, S3Client, Signer};
use sp_runtime::AccountId32;
use std::collections::HashMap;
use std::env;
use storage_client::{AdminClient, ClientConfig, NegotiateRequest, ProviderClient};
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
    let signer = Signer::from_seed("//Bob")?;
    let client = S3Client::new(chain_ws, provider_url, signer.clone()).await?;
    println!("  Client connected successfully");

    // Discover the provider's on-chain account from its /info endpoint
    let provider = ProviderClient::fetch_provider_id(provider_url).await?;
    println!("  Provider account (from /info): {provider}");

    let mut admin = AdminClient::new(
        ClientConfig {
            chain_ws_url: chain_ws.to_string(),
            ..Default::default()
        },
        signer,
    )?;
    admin.connect().await?;
    let nonce = admin.agreement_nonce(&owner).await?;

    // Step 2: Negotiate signed agreement terms
    println!();
    println!("Step 2: Negotiating signed terms with provider...");
    let signed = ProviderClient::negotiate_terms(
        provider_url,
        &NegotiateRequest {
            owner: owner.clone(),
            max_bytes: 1_000_000_000, // 1 GB
            duration: 500,            // 500 blocks
            price_per_byte: 1,
            nonce,
            bucket: None,
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
    let bucket_id = client
        .create_bucket(
            provider,
            signed.terms,
            signed.signature,
            s3_client::Visibility::Private,
        )
        .await?;
    println!("  Bucket created: {bucket_id}");

    let bucket = client.head_bucket(bucket_id).await?;
    assert!(
        bucket.members.iter().any(|m| m.account == owner),
        "Signer must be a member of the new bucket"
    );
    let listed = client.list_buckets(None).await?;
    assert!(
        listed.iter().any(|b| b.bucket_id == bucket_id),
        "New bucket must be in the signer's bucket list"
    );

    // Step 4: Upload objects
    println!();
    println!("Step 4: Uploading objects...");

    let content_1 = b"Hello from S3 CI integration test!";
    let mut metadata_1 = HashMap::new();
    metadata_1.insert("test-key".to_string(), "test-value".to_string());
    let put_result_1 = client
        .put_object(
            bucket_id,
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
            bucket_id,
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

    let get_result_1 = client.get_object(bucket_id, "hello.txt").await?;
    println!(
        "  Downloaded hello.txt ({} bytes, content_type={})",
        get_result_1.size, get_result_1.content_type
    );
    assert_eq!(
        get_result_1.data.as_slice(),
        content_1,
        "Content mismatch for hello.txt"
    );
    assert_eq!(get_result_1.content_type, "text/plain");
    assert_eq!(
        get_result_1.metadata.get("test-key").map(String::as_str),
        Some("test-value"),
        "User metadata mismatch for hello.txt"
    );
    println!("    Content verified!");

    let get_result_2 = client.get_object(bucket_id, "data/payload.bin").await?;
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

    // Step 6: List objects
    println!();
    println!("Step 6: Listing objects...");
    let listing = client
        .list_objects_v2(bucket_id, ListObjectsParams::default())
        .await?;
    let keys: Vec<&str> = listing.contents.iter().map(|o| o.key.as_str()).collect();
    println!("  Keys: {keys:?}");
    assert_eq!(keys, vec!["data/payload.bin", "hello.txt"]);

    // Step 7: Head object
    println!();
    println!("Step 7: Checking object metadata...");
    let head = client.head_object(bucket_id, "hello.txt").await?;
    println!("  hello.txt metadata:");
    println!("    Content-Type: {}", head.content_type);
    println!("    Size: {}", head.size);
    println!("    ETag: {}", head.etag);
    assert_eq!(head.size, content_1.len() as u64);
    assert_eq!(head.cid, put_result_1.cid);

    // Step 8: Delete objects
    println!();
    println!("Step 8: Deleting objects...");

    client.delete_object(bucket_id, "hello.txt").await?;
    println!("  Deleted hello.txt");

    client.delete_object(bucket_id, "data/payload.bin").await?;
    println!("  Deleted data/payload.bin");

    let listing = client
        .list_objects_v2(bucket_id, ListObjectsParams::default())
        .await?;
    assert!(listing.contents.is_empty(), "Bucket index must be empty");

    // Summary
    println!();
    println!("=== PASSED: All S3 tests completed successfully! ===");
    println!();
    println!("Summary:");
    println!("  - Created S3 bucket ({bucket_id})");
    println!("  - Uploaded 2 objects via provider HTTP API");
    println!("  - Downloaded and verified 2 objects");
    println!("  - Listed objects");
    println!("  - Checked object metadata via HEAD");
    println!("  - Deleted all objects");

    Ok(())
}
