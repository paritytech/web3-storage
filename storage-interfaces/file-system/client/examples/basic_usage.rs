// SPDX-License-Identifier: Apache-2.0

//! Basic Usage Example for File System Client
//!
//! This example demonstrates:
//! - Negotiating signed agreement terms with a provider
//! - Creating a drive (atomically opens bucket + primary agreement)
//! - Creating directories
//! - Uploading files
//! - Listing directory contents
//! - Downloading files
//!
//! Prerequisites:
//! 1. Start the blockchain: `just start-chain`
//! 2. Start a provider node: `just start-provider`
//! 3. Provider must be registered with `accepting_primary = true` (run `just demo`
//!    once or register manually via Polkadot.js).
//!
//! Run this example:
//! ```bash
//! cargo run --example basic_usage [chain_ws] [provider_url]
//! ```

use file_system_client::FileSystemClient;
use sp_runtime::AccountId32;
use std::env;
use storage_client::{NegotiateRequest, ProviderClient};
use storage_subxt::subxt_signer::sr25519::dev as dev_signer;

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

    println!("🚀 File System Client - Basic Usage Example\n");
    println!("{}", "=".repeat(60));
    println!("Chain WebSocket: {chain_ws}");
    println!("Provider URL: {provider_url}");

    // === STEP 1: Create the client ===
    println!("\n📡 Step 1: Connecting to blockchain and provider...");

    let mut fs_client = FileSystemClient::new(chain_ws, provider_url)
        .await?
        .with_dev_signer("alice") // Use Alice for testing
        .await?;

    let owner: AccountId32 = dev_signer::alice().public_key().0.into();
    let provider = ProviderClient::fetch_provider_id(provider_url).await?;
    println!("  Provider account (from /info): {provider}");

    println!("✅ Connected successfully!");

    // === STEP 2: Negotiate signed terms with the provider ===
    println!("\n🤝 Step 2: Negotiating signed agreement terms...");

    let signed = ProviderClient::negotiate_terms(
        provider_url,
        &NegotiateRequest {
            owner: owner.clone(),
            max_bytes: 10_000_000_000, // 10 GB
            duration: 500,             // 500 blocks
            price_per_byte: 1,
            replica_params: None,
            bucket_id: None,
        },
    )
    .await?;

    println!(
        "✅ Provider signed terms: nonce={}, valid_until={}",
        signed.terms.nonce, signed.terms.valid_until
    );

    // === STEP 3: Create a drive ===
    println!("\n📁 Step 3: Creating a new drive...");

    let drive_id = fs_client
        .create_drive(
            Some("My Documents"),
            provider,
            signed.terms,
            signed.signature,
        )
        .await?;

    println!("✅ Drive created with ID: {drive_id}");

    // === STEP 4: Create directories ===
    println!("\n📂 Step 4: Creating directory structure...");

    let bucket_id = fs_client.get_bucket_id(drive_id).await?;
    println!("   Associated bucket: ID = {bucket_id}");

    println!("   Creating /documents...");
    fs_client
        .create_directory(drive_id, "/documents", bucket_id)
        .await?;
    println!("   ✅ Created /documents");

    println!("   Creating /documents/work...");
    fs_client
        .create_directory(drive_id, "/documents/work", bucket_id)
        .await?;
    println!("   ✅ Created /documents/work");

    println!("   Creating /photos...");
    fs_client
        .create_directory(drive_id, "/photos", bucket_id)
        .await?;
    println!("   ✅ Created /photos");

    // === STEP 5: Upload files ===
    println!("\n📝 Step 5: Uploading files...");

    let readme_content = b"# My Documents\n\nWelcome to my decentralized file system!\n\nThis is a demo of Layer 1 file system built on Scalable Web3 Storage.";
    println!(
        "   Uploading /README.md ({} bytes)...",
        readme_content.len()
    );
    fs_client
        .upload_file(drive_id, "/README.md", readme_content, bucket_id)
        .await?;
    println!("   ✅ Uploaded /README.md");

    // Upload a file in subdirectory
    let report_content =
        b"Q4 2024 Report\n\n Revenue: $1M\nGrowth: 50%\nCustomers: 1000\n\nStrong quarter!";
    println!(
        "   Uploading /documents/work/report.txt ({} bytes)...",
        report_content.len()
    );
    fs_client
        .upload_file(
            drive_id,
            "/documents/work/report.txt",
            report_content,
            bucket_id,
        )
        .await?;
    println!("   ✅ Uploaded /documents/work/report.txt");

    // Upload another file
    let notes_content = b"Meeting Notes - 2024-12-01\n\n1. Discussed Q4 results\n2. Planning for 2025\n3. New hires approved";
    println!(
        "   Uploading /documents/notes.txt ({} bytes)...",
        notes_content.len()
    );
    fs_client
        .upload_file(drive_id, "/documents/notes.txt", notes_content, bucket_id)
        .await?;
    println!("   ✅ Uploaded /documents/notes.txt");

    // === STEP 6: List directory contents ===
    println!("\n📋 Step 6: Listing directory contents...");

    // List root directory
    println!("\n   Contents of /:");
    let root_entries = fs_client.list_directory(drive_id, "/").await?;
    for entry in root_entries {
        let entry_type = if entry.is_directory() { "📁" } else { "📄" };
        println!(
            "   {} {} ({} bytes)",
            entry_type,
            entry.name_str(),
            entry.size
        );
    }

    // List /documents directory
    println!("\n   Contents of /documents:");
    let docs_entries = fs_client.list_directory(drive_id, "/documents").await?;
    for entry in docs_entries {
        let entry_type = if entry.is_directory() { "📁" } else { "📄" };
        println!(
            "   {} {} ({} bytes)",
            entry_type,
            entry.name_str(),
            entry.size
        );
    }

    // List /documents/work directory
    println!("\n   Contents of /documents/work:");
    let work_entries = fs_client
        .list_directory(drive_id, "/documents/work")
        .await?;
    for entry in work_entries {
        let entry_type = if entry.is_directory() { "📁" } else { "📄" };
        println!(
            "   {} {} ({} bytes)",
            entry_type,
            entry.name_str(),
            entry.size
        );
    }

    // === STEP 7: Download and verify files ===
    println!("\n⬇️  Step 7: Downloading and verifying files...");

    // Download README.md
    println!("\n   Downloading /README.md...");
    let downloaded_readme = fs_client.download_file(drive_id, "/README.md").await?;
    println!("   ✅ Downloaded {} bytes", downloaded_readme.len());

    // Verify content
    if downloaded_readme == readme_content {
        println!("   ✅ Content verified!");
        println!(
            "   Content preview: {}",
            String::from_utf8_lossy(&downloaded_readme[..50])
        );
    } else {
        println!("   ❌ Content mismatch!");
    }

    // Download report
    println!("\n   Downloading /documents/work/report.txt...");
    let downloaded_report = fs_client
        .download_file(drive_id, "/documents/work/report.txt")
        .await?;
    println!("   ✅ Downloaded {} bytes", downloaded_report.len());

    if downloaded_report == report_content {
        println!("   ✅ Content verified!");
        let report_text = String::from_utf8_lossy(&downloaded_report);
        println!(
            "   Content:\n{}",
            report_text
                .lines()
                .take(3)
                .collect::<Vec<_>>()
                .join("\n   ")
        );
    } else {
        println!("   ❌ Content mismatch!");
    }

    // === Summary ===
    println!("\n{}", "=".repeat(60));
    println!("\n🎉 Example completed successfully!");
    println!("\n📊 Summary:");
    println!("   ✅ Created drive: {drive_id}");
    println!("   ✅ Created 3 directories");
    println!("   ✅ Uploaded 3 files");
    println!("   ✅ Listed directory contents");
    println!("   ✅ Downloaded and verified files");
    println!("\n💡 Next steps:");
    println!("   - Try clearing the drive: clear_drive()");
    println!("   - Try deleting the drive: delete_drive()");
    println!("   - Explore more file operations");
    println!("   - Check the on-chain state via polkadot.js");

    Ok(())
}
