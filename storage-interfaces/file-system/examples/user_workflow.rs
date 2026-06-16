// SPDX-License-Identifier: Apache-2.0

//! User Workflow Example: Using storage for files
//!
//! This example demonstrates the user flow in the bucket-based model.
//! Users are responsible for:
//! - Creating drives on their assigned buckets
//! - Uploading and downloading files
//! - Managing folder structures
//!
//! Users do NOT need to manage:
//! - Buckets (admin creates these)
//! - Storage agreements (admin handles these)
//! - Provider failures (admin replaces failed providers)
//! - Challenges (automated by Layer 0)

use file_system_primitives::DriveId;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    println!("==================================================");
    println!("  USER WORKFLOW: Using Storage for Files");
    println!("==================================================\n");

    // NOTE: In production, this would be initialized with actual endpoints
    // let fs_client = FileSystemClient::new(
    //     "ws://127.0.0.1:2222",      // Parachain RPC
    //     "http://provider.example.com", // Storage provider HTTP
    // ).await?;

    println!("Client initialized\n");

    // ============================================================
    // Step 1: List Available Buckets
    // ============================================================

    println!("Step 1: Checking available storage...");

    // Query which buckets the user can access
    // Returns buckets where user has Reader+Writer permissions
    // let buckets = fs_client.list_my_buckets().await?;

    // Placeholder response
    let buckets = vec![
        BucketInfo {
            id: 1,
            capacity_gb: 100,
            available_gb: 100,
            admin: "Alice".to_string(),
        },
    ];

    for bucket in &buckets {
        println!("  Bucket {}: {} GB available", bucket.id, bucket.available_gb);
        println!("    Admin: {}", bucket.admin);
    }
    println!();

    let bucket_id = buckets[0].id;

    // ============================================================
    // Step 2: Create Drive on Bucket
    // ============================================================

    println!("Step 2: Creating drive on bucket {}...", bucket_id);

    // User creates drive on their assigned bucket
    // System automatically validates:
    // - Bucket exists
    // - User has Reader+Writer permissions
    // - Bucket not already used by another drive

    // let drive_id = fs_client.create_drive(bucket_id, Some("My Documents")).await?;

    let drive_id: DriveId = 1; // Placeholder
    println!("✓ Drive {} created", drive_id);
    println!();

    // ============================================================
    // Step 3: Upload Files
    // ============================================================

    println!("Step 3: Uploading files...");

    // Upload a document
    // let file1 = std::fs::read("report.pdf")?;
    // fs_client.upload_file(drive_id, "/report.pdf", &file1, bucket_id).await?;
    println!("✓ Uploaded report.pdf");

    // Upload a presentation
    // let file2 = std::fs::read("presentation.pptx")?;
    // fs_client.upload_file(drive_id, "/presentation.pptx", &file2, bucket_id).await?;
    println!("✓ Uploaded presentation.pptx");

    // Upload to subfolder
    // let file3 = std::fs::read("photo.jpg")?;
    // fs_client.upload_file(drive_id, "/images/photo.jpg", &file3, bucket_id).await?;
    println!("✓ Uploaded /images/photo.jpg");
    println!();

    // ============================================================
    // Step 4: Create Folders
    // ============================================================

    println!("Step 4: Creating folders...");

    // fs_client.create_directory(drive_id, "/documents", bucket_id).await?;
    println!("✓ Created /documents");

    // fs_client.create_directory(drive_id, "/images", bucket_id).await?;
    println!("✓ Created /images");
    println!();

    // ============================================================
    // Step 5: List Directory
    // ============================================================

    println!("Step 5: Listing files:");

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
            println!("  [{}] {} ({} bytes)", type_str, name, size);
        }
    }
    println!();

    // ============================================================
    // Step 6: Download File
    // ============================================================

    println!("Step 6: Downloading file...");

    // let data = fs_client.download_file(drive_id, "/report.pdf").await?;
    // std::fs::write("./downloaded_report.pdf", data)?;
    println!("✓ Downloaded report.pdf");
    println!();

    // ============================================================
    // Step 7: Delete File
    // ============================================================

    println!("Step 7: Deleting old file...");

    // fs_client.delete_file(drive_id, "/old_document.pdf", bucket_id).await?;
    println!("✓ Deleted /old_document.pdf");
    println!();

    // ============================================================
    // Done!
    // ============================================================

    println!("==================================================");
    println!("✓ All operations complete!");
    println!("==================================================");
    println!();
    println!("What the user did:");
    println!("  ✓ Listed available buckets");
    println!("  ✓ Created drive on assigned bucket");
    println!("  ✓ Uploaded files");
    println!("  ✓ Created folders");
    println!("  ✓ Listed directory");
    println!("  ✓ Downloaded file");
    println!("  ✓ Deleted file");
    println!();
    println!("What the user did NOT do:");
    println!("  ✗ Create buckets");
    println!("  ✗ Manage storage agreements");
    println!("  ✗ Handle challenges");
    println!("  ✗ Replace failed providers");
    println!();
    println!("Infrastructure is completely transparent to the user!");
    println!();

    Ok(())
}

// Helper struct for demonstration
#[derive(Debug)]
struct BucketInfo {
    id: u64,
    capacity_gb: u64,
    available_gb: u64,
    admin: String,
}
