//! Basic File System Usage Example
//!
//! This example demonstrates the basic usage of the file system primitives
//! and client SDK for the Layer 1 file system.
//!
//! Run with: `cargo run --example basic_usage`

use file_system_primitives::{compute_cid, DirectoryEntry, DirectoryNode, EntryType, FileManifest};
use sp_runtime::BoundedVec;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== File System Primitives Example ===\n");

    // Example 1: Create an empty root directory
    println!("1. Creating an empty root directory...");
    let root = DirectoryNode::new_empty(1); // drive_id = 1
    let root_cid = root.compute_cid();
    println!("   Root CID: {}", hex::encode(root_cid.as_bytes()));
    println!("   Root has {} children", root.children.len());
    println!();

    // Example 2: Create a directory with some files
    println!("2. Creating a directory with files...");
    let mut documents_dir = DirectoryNode::new_empty(1);

    // Add a text file entry
    let file1_content = b"Hello, Web3 Storage!";
    let file1_cid = compute_cid(file1_content);

    documents_dir
        .add_child(DirectoryEntry {
            name: BoundedVec::try_from(b"hello.txt".to_vec()).unwrap(),
            entry_type: EntryType::File,
            cid: file1_cid,
            size: file1_content.len() as u64,
            mtime: current_timestamp(),
        })
        .expect("Failed to add child");

    // Add a PDF file entry (simulated)
    let file2_cid = compute_cid(b"PDF content goes here...");
    documents_dir
        .add_child(DirectoryEntry {
            name: BoundedVec::try_from(b"report.pdf".to_vec()).unwrap(),
            entry_type: EntryType::File,
            cid: file2_cid,
            size: 1024,
            mtime: current_timestamp(),
        })
        .expect("Failed to add child");

    println!(
        "   Documents directory has {} files:",
        documents_dir.children.len()
    );
    for entry in documents_dir.children.iter() {
        println!("     - {} ({} bytes)", entry.name_str(), entry.size);
    }
    println!();

    // Example 3: Serialize and compute CID
    println!("3. Serializing directory and computing CID...");
    let dir_bytes = documents_dir.to_scale_bytes();
    let dir_cid = documents_dir.compute_cid();
    println!("   Serialized size: {} bytes", dir_bytes.len());
    println!("   Directory CID: {}", hex::encode(dir_cid.as_bytes()));
    println!();

    // Example 4: Deserialize directory
    println!("4. Deserializing directory from bytes...");
    let deserialized_dir = DirectoryNode::from_scale_bytes(&dir_bytes)?;
    println!("   Successfully deserialized!");
    println!("   Children count: {}", deserialized_dir.children.len());
    assert_eq!(
        documents_dir.children.len(),
        deserialized_dir.children.len()
    );
    println!();

    // Example 5: Create a FileManifest with chunks
    println!("5. Creating a FileManifest with chunks...");
    let chunk1_data = vec![0u8; 256 * 1024]; // 256 KiB
    let chunk1_cid = compute_cid(&chunk1_data);

    let chunk2_data = vec![0u8; 128 * 1024]; // 128 KiB
    let chunk2_cid = compute_cid(&chunk2_data);

    let mut manifest = FileManifest {
        drive_id: 1,
        mime_type: BoundedVec::try_from(b"application/pdf".to_vec()).unwrap(),
        total_size: (chunk1_data.len() + chunk2_data.len()) as u64,
        chunks: BoundedVec::default(),
        encryption_params: BoundedVec::default(),
    };
    manifest
        .add_chunk(chunk1_cid, 0)
        .expect("Failed to add chunk");
    manifest
        .add_chunk(chunk2_cid, 1)
        .expect("Failed to add chunk");

    println!(
        "   File size: {} bytes ({} chunks)",
        manifest.total_size,
        manifest.chunks.len()
    );
    for chunk in manifest.chunks.iter() {
        let cid_hex = hex::encode(chunk.cid.as_bytes());
        println!(
            "     - Chunk {}: CID 0x{}...",
            chunk.sequence,
            &cid_hex[..16]
        );
    }

    let manifest_bytes = manifest.to_scale_bytes();
    let manifest_cid = compute_cid(&manifest_bytes);
    println!("   Manifest CID: {}", hex::encode(manifest_cid.as_bytes()));
    println!();

    // Example 6: Build a hierarchical structure
    println!("6. Building a hierarchical file system structure...");
    let mut root_with_structure = DirectoryNode::new_empty(1);

    // Add documents directory
    let docs_cid = documents_dir.compute_cid();
    root_with_structure
        .add_child(DirectoryEntry {
            name: BoundedVec::try_from(b"documents".to_vec()).unwrap(),
            entry_type: EntryType::Directory,
            cid: docs_cid,
            size: 0,
            mtime: current_timestamp(),
        })
        .expect("Failed to add child");

    // Add an empty images directory
    let images_dir = DirectoryNode::new_empty(1);
    let images_cid = images_dir.compute_cid();
    root_with_structure
        .add_child(DirectoryEntry {
            name: BoundedVec::try_from(b"images".to_vec()).unwrap(),
            entry_type: EntryType::Directory,
            cid: images_cid,
            size: 0,
            mtime: current_timestamp(),
        })
        .expect("Failed to add child");

    println!("   Root structure:");
    println!("   /");
    for entry in root_with_structure.children.iter() {
        let entry_type = if entry.is_directory() { "dir" } else { "file" };
        println!("   ├── {} ({})", entry.name_str(), entry_type);
    }

    let final_root_cid = root_with_structure.compute_cid();
    println!(
        "\n   Final root CID: {}",
        hex::encode(final_root_cid.as_bytes())
    );
    println!();

    println!("=== Example Complete ===");
    println!("\nKey Takeaways:");
    println!("- Every change to the file system produces a new root CID");
    println!("- Content-addressed storage means identical content has identical CIDs");
    println!("- Directory structure is a Merkle-DAG (Directed Acyclic Graph)");
    println!("- Each node (file or directory) is identified by its CID");

    Ok(())
}

fn current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}
