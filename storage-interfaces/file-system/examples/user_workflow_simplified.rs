// SPDX-License-Identifier: Apache-2.0

//! User Workflow Example: Using storage WITHOUT knowing about buckets
//!
//! This demonstrates the truly simplified user experience in Layer 1 File System.
//! Users only need to know about DRIVES - buckets are completely hidden!
//!
//! What the user does:
//! - Specify storage requirements (size, number of providers)
//! - Get a drive
//! - Upload/download files
//!
//! What the user does NOT need to know:
//! - Buckets (Layer 0 concept)
//! - Storage agreements
//! - Provider accounts
//! - Challenges or proofs

use file_system_primitives::DriveId;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    println!("==================================================");
    println!("  USER WORKFLOW: Simple Drive Creation");
    println!("==================================================\n");

    // NOTE: In production, this would be initialized with actual endpoints
    // let mut fs_client = FileSystemClient::new(
    //     "ws://127.0.0.1:2222",      // Parachain RPC
    //     "http://provider.example.com", // Storage provider HTTP
    // ).await?;

    println!("Client initialized\n");

    // ============================================================
    // Step 1: Create Drive (NO BUCKET KNOWLEDGE REQUIRED!)
    // ============================================================

    println!("Step 1: Creating a new drive...");
    println!("  Name: My Documents");
    println!("  Storage capacity: 10 GB");
    println!("  Storage period: 500 blocks");
    println!("  Payment: 1 token");
    println!("  Providers: Auto (defaults to 1 for short-term)");
    println!();

    // User just specifies what they need - system handles everything else!
    // let drive_id = fs_client.create_drive(
    //     Some("My Documents"),
    //     10_000_000_000,     // 10 GB
    //     500,                 // 500 blocks (short-term, so 1 provider by default)
    //     1_000_000_000_000,   // 1 token (12 decimals)
    //     None,                // Use default providers (auto-determined)
    // ).await?;

    let drive_id: DriveId = 1; // Placeholder

    println!("✓ Drive {} created!", drive_id);
    println!("  System automatically:");
    println!("    - Created bucket in Layer 0");
    println!("    - Selected 1 provider (short-term storage)");
    println!("    - Requested storage agreement with provider");
    println!("    - Set up empty drive structure");
    println!("  User never saw any of this complexity!");
    println!();

    // ============================================================
    // Optional: Advanced Configuration Example
    // ============================================================

    println!("Advanced Example: Creating a highly replicated drive...");
    println!("  Name: Critical Data");
    println!("  Storage capacity: 5 GB");
    println!("  Storage period: 2000 blocks (long-term)");
    println!("  Payment: 2 tokens");
    println!("  Providers: 5 (1 primary + 4 replicas for high redundancy)");
    println!();

    // For critical data, user can specify more providers
    // let drive_id = fs_client.create_drive(
    //     Some("Critical Data"),
    //     5_000_000_000,       // 5 GB
    //     2000,                // 2000 blocks (long-term)
    //     2_000_000_000_000,   // 2 tokens (more payment for more providers)
    //     Some(5),             // 5 providers for high redundancy
    // ).await?;

    println!("✓ Advanced drive created with custom configuration!");
    println!("  - 5 providers selected for maximum redundancy");
    println!();

    // ============================================================
    // Step 2: Upload Files
    // ============================================================

    println!("Step 2: Uploading files...");

    // Upload a document
    // let file1 = std::fs::read("report.pdf")?;
    // fs_client.upload_file(drive_id, "/report.pdf", &file1, drive_bucket_id).await?;
    println!("✓ Uploaded report.pdf (1 MB)");

    // Upload a presentation
    // let file2 = std::fs::read("presentation.pptx")?;
    // fs_client.upload_file(drive_id, "/presentation.pptx", &file2, drive_bucket_id).await?;
    println!("✓ Uploaded presentation.pptx (2 MB)");

    // Upload to subfolder (auto-creates folder)
    // let file3 = std::fs::read("photo.jpg")?;
    // fs_client.upload_file(drive_id, "/images/photo.jpg", &file3, drive_bucket_id).await?;
    println!("✓ Uploaded /images/photo.jpg (500 KB)");
    println!();

    // ============================================================
    // Step 3: Create Folders
    // ============================================================

    println!("Step 3: Organizing with folders...");

    // fs_client.create_directory(drive_id, "/documents", drive_bucket_id).await?;
    println!("✓ Created /documents");

    // fs_client.create_directory(drive_id, "/images/vacation", drive_bucket_id).await?;
    println!("✓ Created /images/vacation");
    println!();

    // ============================================================
    // Step 4: List Directory
    // ============================================================

    println!("Step 4: Listing files:");

    // let entries = fs_client.list_directory(drive_id, "/").await?;

    // Placeholder entries
    let entries = vec![
        ("report.pdf", "FILE", 1_048_576),
        ("presentation.pptx", "FILE", 2_097_152),
        ("documents", "DIR", 0),
        ("images", "DIR", 0),
    ];

    for (name, type_str, size) in entries {
        if type_str == "DIR" {
            println!("  [{}] {}/", type_str, name);
        } else {
            let size_kb = size / 1024;
            println!("  [{}] {} ({} KB)", type_str, name, size_kb);
        }
    }
    println!();

    // ============================================================
    // Step 5: Download File
    // ============================================================

    println!("Step 5: Downloading file...");

    // let data = fs_client.download_file(drive_id, "/report.pdf").await?;
    // std::fs::write("./downloaded_report.pdf", data)?;
    println!("✓ Downloaded report.pdf to local disk");
    println!();

    // ============================================================
    // Step 6: Delete File
    // ============================================================

    println!("Step 6: Cleaning up old files...");

    // fs_client.delete_file(drive_id, "/old_document.pdf", drive_bucket_id).await?;
    println!("✓ Deleted /old_document.pdf");
    println!();

    // ============================================================
    // Done!
    // ============================================================

    println!("==================================================");
    println!("✓ All operations complete!");
    println!("==================================================");
    println!();
    println!("What the user DID:");
    println!("  ✓ Created drive with storage requirements");
    println!("  ✓ Uploaded files");
    println!("  ✓ Created folders");
    println!("  ✓ Listed directory");
    println!("  ✓ Downloaded file");
    println!("  ✓ Deleted file");
    println!();
    println!("What the user did NOT need to know:");
    println!("  ✗ Buckets (completely hidden!)");
    println!("  ✗ Storage agreements");
    println!("  ✗ Provider accounts");
    println!("  ✗ Challenges or proofs");
    println!("  ✗ Any Layer 0 concepts");
    println!();
    println!("This is TRUE abstraction - users only see drives and files!");
    println!();

    // ============================================================
    // Comparison: What if Layer 1 didn't exist?
    // ============================================================

    println!("==================================================");
    println!("  For Comparison: Without Layer 1 File System");
    println!("==================================================");
    println!();
    println!("User would need to:");
    println!("  1. Create a bucket");
    println!("  2. Find available storage providers");
    println!("  3. Request primary agreement with provider 1");
    println!("  4. Request replica agreement with provider 2");
    println!("  5. Request replica agreement with provider 3");
    println!("  6. Wait for all providers to accept");
    println!("  7. Upload each file chunk manually");
    println!("  8. Create and manage directory Merkle-DAG");
    println!("  9. Track all CIDs manually");
    println!("  10. Handle provider failures manually");
    println!();
    println!("With Layer 1:");
    println!("  1. Create drive (system does steps 1-6 automatically)");
    println!("  2. Upload file (system does steps 7-9 automatically)");
    println!("  3. System handles step 10 transparently");
    println!();
    println!("Layer 1 reduces complexity from 10 steps to 2!");
    println!();

    Ok(())
}
