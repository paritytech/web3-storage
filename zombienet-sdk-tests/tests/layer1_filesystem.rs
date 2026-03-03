//! Layer 1 - File System Integration Tests
//!
//! Tests the full file system workflow:
//! 1. Spawn network + provider (via common helpers)
//! 2. Register provider on-chain
//! 3. Create a drive (which sets up bucket + agreement internally)
//! 4. Create directories
//! 5. Upload files
//! 6. List and verify directories
//! 7. Download and verify file contents

use crate::common::{
    config::*,
    network::{build_network_config, spawn_network, wait_for_collator_ws},
    provider::ProviderProcess,
    setup::register_alice_provider,
};
use anyhow::{Context, Result};
use file_system_client::FileSystemClient;
use file_system_primitives::{CommitStrategy, DirectoryEntry};

async fn list_and_verify(
    fs_client: &mut FileSystemClient,
    drive_id: u64,
    path: &str,
    expected_count: usize,
) -> Result<Vec<DirectoryEntry>> {
    let entries = fs_client
        .list_directory(drive_id, path)
        .await
        .with_context(|| format!("Failed to list directory '{}'", path))?;

    log::info!("  {} entries: {}", path, entries.len());
    for entry in &entries {
        let kind = if entry.is_directory() { "DIR " } else { "FILE" };
        log::info!("    [{}] {} ({} bytes)", kind, entry.name_str(), entry.size);
    }

    assert_eq!(
        entries.len(),
        expected_count,
        "Expected {} entries in {}, got {}",
        expected_count,
        path,
        entries.len()
    );

    Ok(entries)
}

#[tokio::test(flavor = "multi_thread")]
async fn filesystem_integration_test() -> Result<()> {
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .try_init();

    log::info!("=== Layer 1: File System Integration Test ===");

    // Step 0: Spawn network + provider
    log::info!("Step 0: Spawning network and provider...");
    let config = build_network_config()?;
    let network = spawn_network(config).await?;
    let chain_ws = wait_for_collator_ws(&network, "collator-alice").await?;
    let _provider = ProviderProcess::spawn(&chain_ws).await?;
    let provider_url = provider_url();

    log::info!("  Chain: {}", chain_ws);
    log::info!("  Provider: {}", provider_url);

    // Step 1: Register provider on-chain (required before create_drive can find providers)
    log::info!("Step 1: Registering provider on-chain...");
    let _ = register_alice_provider(&chain_ws, &provider_url).await?;

    // Step 2: Create the file system client
    log::info!("Step 2: Creating file system client...");
    let mut fs_client = FileSystemClient::new(&chain_ws, &provider_url)
        .await
        .context("Failed to create FileSystemClient")?
        .with_dev_signer("alice")
        .await
        .context("Failed to set dev signer on FileSystemClient")?;
    log::info!("  Client connected successfully");

    // Step 3: Create a drive
    log::info!("Step 3: Creating drive...");
    let drive_id = fs_client
        .create_drive(
            Some("Zombienet SDK Test Drive"),
            AGREEMENT_MAX_BYTES, // 1 GB capacity
            500,                 // 500 blocks duration
            AGREEMENT_MAX_PAYMENT,
            Some(1), // 1 provider minimum
            Some(CommitStrategy::Immediate),
        )
        .await
        .context("Failed to create drive")?;
    log::info!("  Drive created: ID = {}", drive_id);

    let bucket_id = fs_client
        .get_bucket_id(drive_id)
        .await
        .context("Failed to get bucket ID for drive")?;
    log::info!("  Associated bucket: ID = {}", bucket_id);

    // Step 4: Create directories
    log::info!("Step 4: Creating directories...");
    fs_client
        .create_directory(drive_id, "/test-dir", bucket_id)
        .await
        .context("Failed to create /test-dir")?;
    log::info!("  Created /test-dir");

    fs_client
        .create_directory(drive_id, "/test-dir/subdir", bucket_id)
        .await
        .context("Failed to create /test-dir/subdir")?;
    log::info!("  Created /test-dir/subdir");

    // Step 5: Upload files
    log::info!("Step 5: Uploading files...");

    let test_content_1 = b"Hello from zombienet-sdk integration test!";
    fs_client
        .upload_file(drive_id, "/test-dir/hello.txt", test_content_1, bucket_id)
        .await
        .context("Failed to upload /test-dir/hello.txt")?;
    log::info!(
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
        .await
        .context("Failed to upload /test-dir/subdir/nested.txt")?;
    log::info!(
        "  Uploaded /test-dir/subdir/nested.txt ({} bytes)",
        test_content_2.len()
    );

    // Step 6: List directories
    log::info!("Step 6: Listing directories...");

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
    log::info!("Step 7: Downloading and verifying files...");

    let downloaded_1 = fs_client
        .download_file(drive_id, "/test-dir/hello.txt")
        .await
        .context("Failed to download /test-dir/hello.txt")?;
    log::info!(
        "  Downloaded /test-dir/hello.txt ({} bytes)",
        downloaded_1.len()
    );
    assert_eq!(
        downloaded_1.as_slice(),
        test_content_1,
        "Content mismatch for hello.txt"
    );
    log::info!("    Content verified!");

    let downloaded_2 = fs_client
        .download_file(drive_id, "/test-dir/subdir/nested.txt")
        .await
        .context("Failed to download /test-dir/subdir/nested.txt")?;
    log::info!(
        "  Downloaded /test-dir/subdir/nested.txt ({} bytes)",
        downloaded_2.len()
    );
    assert_eq!(
        downloaded_2.as_slice(),
        test_content_2,
        "Content mismatch for nested.txt"
    );
    log::info!("    Content verified!");

    // Summary
    log::info!("=== PASSED: File System Integration Test ===");
    log::info!("  - Created drive (ID: {})", drive_id);
    log::info!("  - Created 2 directories");
    log::info!("  - Uploaded 2 files");
    log::info!("  - Listed 3 directories");
    log::info!("  - Downloaded and verified 2 files");

    Ok(())
}
