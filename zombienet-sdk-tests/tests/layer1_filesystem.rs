//! Layer 1 - File System Integration Tests
//!
//! Tests the full file system workflow:
//! 1. Spawn network + provider (via common helpers)
//! 2. Create a drive (which sets up bucket + agreement internally)
//! 3. Create directories
//! 4. Upload files
//! 5. List and verify directories
//! 6. Download and verify file contents

use crate::common::{
    config::provider_url,
    network::{build_network_config, spawn_network, wait_for_collator_ws},
    provider::ProviderProcess,
};
use anyhow::{anyhow, Context, Result};
use file_system_client::FileSystemClient;
use file_system_primitives::{CommitStrategy, DirectoryEntry};
use storage_client::{ClientConfig, ProviderClient};

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
    let _ = env_logger::Builder::from_env(
        env_logger::Env::default()
            .default_filter_or("off,layer1_filesystem=info,zombienet_sdk=info"),
    )
    .try_init();

    log::info!("=== Layer 1: File System Integration Test ===");

    // Step 0: Spawn network + provider
    log::info!("Step 0: Spawning network and provider...");
    let config = build_network_config()?;
    let network = spawn_network(config).await?;
    let chain_ws = wait_for_collator_ws(&network, "collator-alice").await?;
    let _provider = ProviderProcess::spawn(&chain_ws).await?;
    let provider_endpoint = provider_url();

    log::info!("  Chain: {}", chain_ws);
    log::info!("  Provider: {}", provider_endpoint);

    // Step 1: Register provider on-chain (required before create_drive can find providers)
    log::info!("Step 1: Registering provider on-chain...");
    let client_config = ClientConfig {
        chain_ws_url: chain_ws.clone(),
        provider_urls: vec![provider_endpoint.clone()],
        timeout_secs: 60,
        enable_retries: true,
    };

    let alice_ss58 = "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY";
    // Alice's sr25519 public key (well-known dev account)
    let alice_public_key: Vec<u8> = vec![
        0xd4, 0x35, 0x93, 0xc7, 0x15, 0xfd, 0xd3, 0x1c, 0x61, 0x14, 0x1a, 0xbd, 0x04, 0xa9, 0x9f,
        0xd6, 0x82, 0x2c, 0x85, 0x58, 0x85, 0x4c, 0xcd, 0xe3, 0x9a, 0x56, 0x84, 0xe7, 0xa5, 0x6d,
        0xa2, 0x7d,
    ];

    let mut alice_provider = ProviderClient::new(client_config, alice_ss58.to_string())
        .map_err(|e| anyhow!("Failed to create ProviderClient: {}", e))?;
    alice_provider
        .connect()
        .await
        .map_err(|e| anyhow!("ProviderClient connect failed: {}", e))?;
    alice_provider
        .set_dev_signer("alice")
        .map_err(|e| anyhow!("ProviderClient set_dev_signer failed: {}", e))?;

    alice_provider
        .register(
            "/ip4/127.0.0.1/tcp/3333".to_string(),
            alice_public_key,
            1_000_000_000_000_000, // 1000 tokens stake
        )
        .await
        .map_err(|e| anyhow!("register_provider failed: {}", e))?;
    log::info!("  Provider registered");

    // Step 2: Create the file system client
    log::info!("Step 2: Creating file system client...");
    let mut fs_client = FileSystemClient::new(&chain_ws, &provider_endpoint)
        .await?
        .with_dev_signer("alice")
        .await?;
    log::info!("  Client connected successfully");

    // Step 3: Create a drive
    log::info!("Step 3: Creating drive...");
    let drive_id = fs_client
        .create_drive(
            Some("Zombienet SDK Test Drive"),
            1_000_000_000,         // 1 GB capacity
            500,                   // 500 blocks duration
            1_000_000_000_000_000, // 1000 tokens payment (12 decimals)
            Some(1),               // 1 provider minimum
            Some(CommitStrategy::Immediate),
        )
        .await?;
    log::info!("  Drive created: ID = {}", drive_id);

    let bucket_id = fs_client.get_bucket_id(drive_id).await?;
    log::info!("  Associated bucket: ID = {}", bucket_id);

    // Step 4: Create directories
    log::info!("Step 4: Creating directories...");
    fs_client
        .create_directory(drive_id, "/test-dir", bucket_id)
        .await?;
    log::info!("  Created /test-dir");

    fs_client
        .create_directory(drive_id, "/test-dir/subdir", bucket_id)
        .await?;
    log::info!("  Created /test-dir/subdir");

    // Step 5: Upload files
    log::info!("Step 5: Uploading files...");

    let test_content_1 = b"Hello from zombienet-sdk integration test!";
    fs_client
        .upload_file(drive_id, "/test-dir/hello.txt", test_content_1, bucket_id)
        .await?;
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
        .await?;
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
        .await?;
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
        .await?;
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
