//! Basic File System Usage Example
//!
//! This example demonstrates the basic usage of the file system primitives
//! and client SDK for the Layer 1 file system.
//!
//! Run with: `cargo run --example basic_usage`

use file_system_primitives::{compute_cid, DirectoryEntry, DirectoryNode, EntryType, FileManifest};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== File System Primitives Example ===\n");

    // Example 1: Create an empty root directory
    println!("1. Creating an empty root directory...");
    let root = DirectoryNode::new_empty("my_drive".to_string());
    let root_cid = root.compute_cid()?;
    println!("   Root CID: {}", hex::encode(root_cid.as_bytes()));
    println!("   Root has {} children", root.children.len());
    println!();

    // Example 2: Create a directory with some files
    println!("2. Creating a directory with files...");
    let mut documents_dir = DirectoryNode::new_empty("documents".to_string());

    // Add a text file entry
    let file1_content = b"Hello, Web3 Storage!";
    let file1_cid = compute_cid(file1_content);

    documents_dir.children.push(DirectoryEntry {
        name: "hello.txt".to_string(),
        r#type: EntryType::File.into(),
        cid: format!("0x{}", hex::encode(file1_cid.as_bytes())),
        size: file1_content.len() as u64,
        mtime: current_timestamp(),
    });

    // Add a PDF file entry (simulated)
    let file2_cid = compute_cid(b"PDF content goes here...");
    documents_dir.children.push(DirectoryEntry {
        name: "report.pdf".to_string(),
        r#type: EntryType::File.into(),
        cid: format!("0x{}", hex::encode(file2_cid.as_bytes())),
        size: 1024,
        mtime: current_timestamp(),
    });

    println!("   Documents directory has {} files:", documents_dir.children.len());
    for entry in &documents_dir.children {
        println!("     - {} ({} bytes)", entry.name, entry.size);
    }
    println!();

    // Example 3: Serialize and compute CID
    println!("3. Serializing directory and computing CID...");
    let dir_bytes = documents_dir.to_bytes()?;
    let dir_cid = documents_dir.compute_cid()?;
    println!("   Serialized size: {} bytes", dir_bytes.len());
    println!("   Directory CID: {}", hex::encode(dir_cid.as_bytes()));
    println!();

    // Example 4: Deserialize directory
    println!("4. Deserializing directory from bytes...");
    let deserialized_dir = DirectoryNode::from_bytes(&dir_bytes)?;
    println!("   Successfully deserialized!");
    println!("   Children count: {}", deserialized_dir.children.len());
    assert_eq!(documents_dir.children.len(), deserialized_dir.children.len());
    println!();

    // Example 5: Create a FileManifest with chunks
    println!("5. Creating a FileManifest with chunks...");
    let chunk1_data = vec![0u8; 256 * 1024]; // 256 KiB
    let chunk1_cid = compute_cid(&chunk1_data);

    let chunk2_data = vec![0u8; 128 * 1024]; // 128 KiB
    let chunk2_cid = compute_cid(&chunk2_data);

    let manifest = FileManifest {
        drive_id: "1".to_string(),
        mime_type: "application/pdf".to_string(),
        total_size: (chunk1_data.len() + chunk2_data.len()) as u64,
        chunks: vec![
            file_system_primitives::FileChunk {
                cid: format!("0x{}", hex::encode(chunk1_cid.as_bytes())),
                sequence: 0,
            },
            file_system_primitives::FileChunk {
                cid: format!("0x{}", hex::encode(chunk2_cid.as_bytes())),
                sequence: 1,
            },
        ],
        encryption_params: "".to_string(),
    };

    println!("   File size: {} bytes ({} chunks)", manifest.total_size, manifest.chunks.len());
    for chunk in &manifest.chunks {
        println!("     - Chunk {}: CID {}...", chunk.sequence, &chunk.cid[..18]);
    }

    let manifest_bytes = manifest.to_bytes()?;
    let manifest_cid = compute_cid(&manifest_bytes);
    println!("   Manifest CID: {}", hex::encode(manifest_cid.as_bytes()));
    println!();

    // Example 6: Build a hierarchical structure
    println!("6. Building a hierarchical file system structure...");
    let mut root_with_structure = DirectoryNode::new_empty("root".to_string());

    // Add documents directory
    let docs_cid = documents_dir.compute_cid()?;
    root_with_structure.children.push(DirectoryEntry {
        name: "documents".to_string(),
        r#type: EntryType::Directory.into(),
        cid: format!("0x{}", hex::encode(docs_cid.as_bytes())),
        size: 0,
        mtime: current_timestamp(),
    });

    // Add an empty images directory
    let images_dir = DirectoryNode::new_empty("images".to_string());
    let images_cid = images_dir.compute_cid()?;
    root_with_structure.children.push(DirectoryEntry {
        name: "images".to_string(),
        r#type: EntryType::Directory.into(),
        cid: format!("0x{}", hex::encode(images_cid.as_bytes())),
        size: 0,
        mtime: current_timestamp(),
    });

    println!("   Root structure:");
    println!("   /");
    for entry in &root_with_structure.children {
        let entry_type = if entry.r#type == EntryType::Directory.into() {
            "dir"
        } else {
            "file"
        };
        println!("   ├── {} ({})", entry.name, entry_type);
    }

    let final_root_cid = root_with_structure.compute_cid()?;
    println!("\n   Final root CID: {}", hex::encode(final_root_cid.as_bytes()));
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
