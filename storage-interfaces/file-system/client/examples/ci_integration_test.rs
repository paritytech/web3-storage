//! File System CI Integration Test
//!
//! This test is designed to run in CI after the infrastructure is set up.
//! It tests the full file system workflow:
//! 1. Create a drive (which creates bucket + agreement internally)
//! 2. Create directories
//! 3. Upload files
//! 4. List directories
//! 5. Download and verify files
//!
//! Usage: cargo run --example ci_integration_test <chain_ws> <provider_url>
//!
//! Run via justfile (recommended): just fs-demo-ci

use file_system_client::FileSystemClient;
use file_system_primitives::CommitStrategy;
use std::env;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

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

    // Step 2: Create a drive
    println!();
    println!("Step 2: Creating drive...");
    let drive_id = fs_client
        .create_drive(
            Some("CI Test Drive"),
            1_000_000_000,         // 1 GB capacity
            500,                   // 500 blocks duration
            1_000_000_000_000_000, // 1000 tokens payment (12 decimals)
            Some(1),               // 1 provider minimum
            Some(CommitStrategy::Immediate),
        )
        .await?;
    println!("  Drive created: ID = {drive_id}");

    // Get the bucket_id for this drive
    let bucket_id = fs_client.get_bucket_id(drive_id).await?;
    println!("  Associated bucket: ID = {bucket_id}");

    // Step 3: Create directories
    println!();
    println!("Step 3: Creating directories...");
    fs_client
        .create_directory(drive_id, "/test-dir", bucket_id)
        .await?;
    println!("  Created /test-dir");

    fs_client
        .create_directory(drive_id, "/test-dir/subdir", bucket_id)
        .await?;
    println!("  Created /test-dir/subdir");

    // Step 4: Upload files
    println!();
    println!("Step 4: Uploading files...");

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

    // Step 5: List directories
    println!();
    println!("Step 5: Listing directories...");

    let root_entries = fs_client.list_directory(drive_id, "/").await?;
    println!("  Root directory entries: {}", root_entries.len());
    for entry in &root_entries {
        let entry_type = if entry.is_directory() { "DIR " } else { "FILE" };
        println!(
            "    [{entry_type}] {} ({} bytes)",
            entry.name_str(),
            entry.size
        );
    }
    assert_eq!(root_entries.len(), 1, "Expected 1 entry in root");
    assert!(
        root_entries[0].is_directory(),
        "Expected test-dir to be a directory"
    );
    assert_eq!(
        root_entries[0].name_str(),
        "test-dir",
        "Expected entry named 'test-dir'"
    );

    let test_dir_entries = fs_client.list_directory(drive_id, "/test-dir").await?;
    println!("  /test-dir entries: {}", test_dir_entries.len());
    for entry in &test_dir_entries {
        let entry_type = if entry.is_directory() { "DIR " } else { "FILE" };
        println!(
            "    [{entry_type}] {} ({} bytes)",
            entry.name_str(),
            entry.size
        );
    }
    assert_eq!(test_dir_entries.len(), 2, "Expected 2 entries in /test-dir");

    let subdir_entries = fs_client
        .list_directory(drive_id, "/test-dir/subdir")
        .await?;
    println!("  /test-dir/subdir entries: {}", subdir_entries.len());
    for entry in &subdir_entries {
        let entry_type = if entry.is_directory() { "DIR " } else { "FILE" };
        println!(
            "    [{entry_type}] {} ({} bytes)",
            entry.name_str(),
            entry.size
        );
    }
    assert_eq!(
        subdir_entries.len(),
        1,
        "Expected 1 entry in /test-dir/subdir"
    );

    // Step 6: Download and verify files
    println!();
    println!("Step 6: Downloading and verifying files...");

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
