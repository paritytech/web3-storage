//! File System CI Integration Test
//!
//! This test is designed to run in CI after the infrastructure is set up.
//! It tests the full file system workflow:
//! 1. Negotiate signed agreement terms with the provider
//! 2. Create a drive (atomically opens bucket + primary agreement)
//! 3. Create directories
//! 4. Upload files
//! 5. List directories
//! 6. Download and verify files
//!
//! Usage: cargo run --example ci_integration_test <chain_ws> <provider_url>
//!
//! Run via justfile (recommended): just fs-demo-ci

use file_system_client::FileSystemClient;
use file_system_primitives::DirectoryEntry;
use sp_runtime::AccountId32;
use std::env;
use storage_client::{NegotiateRequest, ProviderClient};
use subxt_signer::sr25519::dev as dev_signer;

async fn list_and_verify(
    fs_client: &mut FileSystemClient,
    drive_id: u64,
    path: &str,
    expected_count: usize,
) -> Result<Vec<DirectoryEntry>, Box<dyn std::error::Error>> {
    let entries = fs_client.list_directory(drive_id, path).await?;
    println!("  {path} entries: {}", entries.len());
    for entry in &entries {
        let kind = if entry.is_directory() { "DIR " } else { "FILE" };
        println!("    [{kind}] {} ({} bytes)", entry.name_str(), entry.size);
    }
    assert_eq!(
        entries.len(),
        expected_count,
        "Expected {expected_count} entries in {path}"
    );
    Ok(entries)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    tracing_subscriber::fmt::init();

    // Parse command line arguments (required)
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: {} <chain_ws> <provider_url>", args[0]);
        eprintln!("  Run via justfile: just fs-demo-ci");
        std::process::exit(1);
    }
    let chain_ws = &args[1];
    let provider_url = &args[2];

    println!("=== File System CI Integration Test ===");
    println!();
    println!("Chain WebSocket: {chain_ws}");
    println!("Provider URL: {provider_url}");
    println!();

    // Step 1: Create the client
    println!("Step 1: Creating file system client...");
    let mut fs_client = FileSystemClient::new(chain_ws, provider_url)
        .await?
        .with_dev_signer("alice")
        .await?;
    println!("  Client connected successfully");

    let owner: AccountId32 = dev_signer::alice().public_key().0.into();

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
            price_per_byte: 0,
            replica_params: None,
            bucket_id: None,
        },
    )
    .await?;
    println!(
        "  Provider signed terms: nonce={}, valid_until={}",
        signed.terms.nonce, signed.terms.valid_until
    );

    // Step 3: Create a drive
    println!();
    println!("Step 3: Creating drive...");
    let drive_id = fs_client
        .create_drive(
            Some("CI Test Drive"),
            provider,
            signed.terms,
            signed.signature,
        )
        .await?;
    println!("  Drive created: ID = {drive_id}");

    let bucket_id = fs_client.get_bucket_id(drive_id).await?;
    println!("  Associated bucket: ID = {bucket_id}");

    // Step 4: Create directories
    println!();
    println!("Step 4: Creating directories...");
    fs_client
        .create_directory(drive_id, "/test-dir", bucket_id)
        .await?;
    println!("  Created /test-dir");

    fs_client
        .create_directory(drive_id, "/test-dir/subdir", bucket_id)
        .await?;
    println!("  Created /test-dir/subdir");

    // Step 5: Upload files
    println!();
    println!("Step 5: Uploading files...");

    let test_content_1 = b"Hello from CI integration test!";
    fs_client
        .upload_file(drive_id, "/test-dir/hello.txt", test_content_1, bucket_id)
        .await?;
    println!(
        "  Uploaded /test-dir/hello.txt ({} bytes)",
        test_content_1.len()
    );

    let test_content_2 = b"This is a nested file in the subdirectory.";
    fs_client
        .upload_file(
            drive_id,
            "/test-dir/subdir/nested.txt",
            test_content_2,
            bucket_id,
        )
        .await?;
    println!(
        "  Uploaded /test-dir/subdir/nested.txt ({} bytes)",
        test_content_2.len()
    );

    // Step 6: List directories
    println!();
    println!("Step 6: Listing directories...");

    let root_entries = list_and_verify(&mut fs_client, drive_id, "/", 1).await?;
    assert!(
        root_entries[0].is_directory(),
        "Expected test-dir to be a directory"
    );
    assert_eq!(
        root_entries[0].name_str(),
        "test-dir",
        "Expected entry named 'test-dir'"
    );

    list_and_verify(&mut fs_client, drive_id, "/test-dir", 2).await?;
    list_and_verify(&mut fs_client, drive_id, "/test-dir/subdir", 1).await?;

    // Step 7: Download and verify files
    println!();
    println!("Step 7: Downloading and verifying files...");

    let downloaded_1 = fs_client
        .download_file(drive_id, "/test-dir/hello.txt")
        .await?;
    println!(
        "  Downloaded /test-dir/hello.txt ({} bytes)",
        downloaded_1.len()
    );
    assert_eq!(
        downloaded_1.as_slice(),
        test_content_1,
        "Content mismatch for hello.txt"
    );
    println!("    Content verified!");

    let downloaded_2 = fs_client
        .download_file(drive_id, "/test-dir/subdir/nested.txt")
        .await?;
    println!(
        "  Downloaded /test-dir/subdir/nested.txt ({} bytes)",
        downloaded_2.len()
    );
    assert_eq!(
        downloaded_2.as_slice(),
        test_content_2,
        "Content mismatch for nested.txt"
    );
    println!("    Content verified!");

    // Summary
    println!();
    println!("=== PASSED: All tests completed successfully! ===");
    println!();
    println!("Summary:");
    println!("  - Created drive (ID: {drive_id})");
    println!("  - Created 2 directories");
    println!("  - Uploaded 2 files");
    println!("  - Listed 3 directories");
    println!("  - Downloaded and verified 2 files");

    Ok(())
}
