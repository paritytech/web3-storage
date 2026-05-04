//! Basic Usage Example for File System Client
//!
//! This example demonstrates:
//! - Creating a drive with storage infrastructure
//! - Creating directories
//! - Uploading files
//! - Listing directory contents
//! - Downloading files
//!
//! Prerequisites:
//! 1. Start the blockchain: `just start-chain`
//! 2. Start a provider node: `just start-provider`
//! 3. With both running, run `just demo` once to register the provider and
//!    open an agreement (or do the equivalent steps manually in Polkadot.js).
//!
//! Run this example:
//! ```bash
//! cargo run --example basic_usage
//! ```

use file_system_client::FileSystemClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    env_logger::init();

    println!("🚀 File System Client - Basic Usage Example\n");
    println!("{}", "=".repeat(60));

    // === STEP 1: Create the client ===
    println!("\n📡 Step 1: Connecting to blockchain and provider...");

    let mut fs_client = FileSystemClient::new(
        "ws://127.0.0.1:2222",   // Parachain WebSocket endpoint
        "http://127.0.0.1:3333", // Provider HTTP endpoint
    )
    .await?
    .with_dev_signer("alice") // Use Alice for testing
    .await?;

    println!("✅ Connected successfully!");

    // === STEP 2: Create a drive ===
    println!("\n📁 Step 2: Creating a new drive...");

    let drive_id = fs_client
        .create_drive(
            Some("My Documents"), // Drive name
            10_000_000_000,       // 10 GB capacity
            500,                  // 500 blocks duration
            1_000_000_000_000,    // 1 token payment (12 decimals)
            None,                 // Auto-determine providers
        )
        .await?;

    println!("✅ Drive created with ID: {drive_id}");
    println!("   Name: My Documents");
    println!("   Capacity: 10 GB");
    println!("   Duration: 500 blocks");

    // Note: In a real scenario, you'd need to wait for bucket creation and agreement setup
    // For this example, we'll assume that's done (via manual setup or scripts)

    // === STEP 3: Create directories ===
    println!("\n📂 Step 3: Creating directory structure...");

    // Get bucket_id from drive (you'd normally query this from chain)
    // For now, we use a placeholder
    let bucket_id = 1u64; // This should come from the drive info

    // Create /documents directory
    println!("   Creating /documents...");
    fs_client
        .create_directory(drive_id, "/documents", bucket_id)
        .await?;
    println!("   ✅ Created /documents");

    // Create /documents/work subdirectory
    println!("   Creating /documents/work...");
    fs_client
        .create_directory(drive_id, "/documents/work", bucket_id)
        .await?;
    println!("   ✅ Created /documents/work");

    // Create /photos directory
    println!("   Creating /photos...");
    fs_client
        .create_directory(drive_id, "/photos", bucket_id)
        .await?;
    println!("   ✅ Created /photos");

    // === STEP 4: Upload files ===
    println!("\n📝 Step 4: Uploading files...");

    // Upload a text file
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

    // === STEP 5: List directory contents ===
    println!("\n📋 Step 5: Listing directory contents...");

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

    // === STEP 6: Download and verify files ===
    println!("\n⬇️  Step 6: Downloading and verifying files...");

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
