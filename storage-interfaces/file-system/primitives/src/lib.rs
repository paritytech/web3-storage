//! File System Primitives for Layer 1
//!
//! This crate provides the core data structures for the Layer 1 file system
//! built on top of Layer 0 (Scalable Web3 Storage).
//!
//! # Architecture
//!
//! - **Layer 0**: Raw blob storage in buckets (content-addressed chunks)
//! - **Layer 1**: File system metadata (directories, file manifests)
//! - **Layer 2**: User interfaces (FUSE, web UI, CLI)
//!
//! # Key Concepts
//!
//! - **Drive**: A user's logical file system, mapped to a Layer 0 bucket
//! - **RootCID**: The content ID of the root directory, stored on-chain
//! - **DirectoryNode**: A directory containing references to children
//! - **FileManifest**: Metadata about a file and its chunks
//! - **CID**: Content Identifier (blake2-256 hash)

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::{string::String, vec::Vec};
use codec::{Decode, Encode, MaxEncodedLen};
use scale_info::TypeInfo;
use sp_core::H256;
use sp_runtime::{traits::Get, BoundedVec, RuntimeDebug};

#[cfg(feature = "std")]
use prost::Message;

// Include the protobuf-generated types
pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/filesystem.rs"));
}

pub use proto::{DirectoryEntry, DirectoryNode, EntryType, FileChunk, FileManifest};

/// Drive identifier (unique ID for each drive)
pub type DriveId = u64;

/// Content Identifier (blake2-256 hash)
pub type Cid = H256;

/// Error types for file system operations
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "std", derive(thiserror::Error))]
pub enum FileSystemError {
    #[cfg_attr(feature = "std", error("Invalid CID format"))]
    InvalidCid,

    #[cfg_attr(feature = "std", error("Serialization failed"))]
    SerializationError,

    #[cfg_attr(feature = "std", error("Deserialization failed"))]
    DeserializationError,

    #[cfg_attr(feature = "std", error("Entry not found: {0}"))]
    EntryNotFound(String),

    #[cfg_attr(feature = "std", error("Invalid path"))]
    InvalidPath,

    #[cfg_attr(feature = "std", error("Not a directory"))]
    NotADirectory,

    #[cfg_attr(feature = "std", error("Not a file"))]
    NotAFile,
}

/// Drive information stored on-chain
#[derive(Clone, Encode, Decode, Eq, PartialEq, RuntimeDebug, TypeInfo, MaxEncodedLen)]
#[scale_info(skip_type_params(MaxNameLength))]
#[codec(mel_bound())]
pub struct DriveInfo<
    AccountId: Encode + Decode + MaxEncodedLen,
    BlockNumber: Encode + Decode + MaxEncodedLen,
    MaxNameLength: Get<u32>,
> {
    /// Owner of the drive
    pub owner: AccountId,
    /// Layer 0 bucket ID where drive data is stored
    pub bucket_id: u64,
    /// Current root CID (content ID of root directory)
    pub root_cid: Cid,
    /// Block number when drive was created
    pub created_at: BlockNumber,
    /// Optional human-readable name (bounded)
    pub name: Option<BoundedVec<u8, MaxNameLength>>,
}

/// Helper functions for working with protobuf types
impl DirectoryNode {
    /// Create a new empty directory
    #[cfg(feature = "std")]
    pub fn new_empty(drive_id: String) -> Self {
        Self {
            drive_id,
            children: Vec::new(),
            metadata: Default::default(),
        }
    }

    /// Add a child entry
    #[cfg(feature = "std")]
    pub fn add_child(&mut self, entry: DirectoryEntry) {
        self.children.push(entry);
    }

    /// Find a child by name
    #[cfg(feature = "std")]
    pub fn find_child(&self, name: &str) -> Option<&DirectoryEntry> {
        self.children.iter().find(|e| e.name == name)
    }

    /// Remove a child by name
    #[cfg(feature = "std")]
    pub fn remove_child(&mut self, name: &str) -> Option<DirectoryEntry> {
        if let Some(pos) = self.children.iter().position(|e| e.name == name) {
            Some(self.children.remove(pos))
        } else {
            None
        }
    }

    /// Serialize to protobuf bytes
    #[cfg(feature = "std")]
    pub fn to_bytes(&self) -> Result<Vec<u8>, FileSystemError> {
        let mut buf = Vec::new();
        self.encode(&mut buf)
            .map_err(|_| FileSystemError::SerializationError)?;
        Ok(buf)
    }

    /// Deserialize from protobuf bytes
    #[cfg(feature = "std")]
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, FileSystemError> {
        Self::decode(bytes).map_err(|_| FileSystemError::DeserializationError)
    }

    /// Compute the CID (blake2-256 hash) of this directory node
    #[cfg(feature = "std")]
    pub fn compute_cid(&self) -> Result<Cid, FileSystemError> {
        let bytes = self.to_bytes()?;
        Ok(compute_cid(&bytes))
    }
}

impl FileManifest {
    /// Create a new file manifest
    #[cfg(feature = "std")]
    pub fn new(drive_id: String, mime_type: String, total_size: u64) -> Self {
        Self {
            drive_id,
            mime_type,
            total_size,
            chunks: Vec::new(),
            encryption_params: String::new(),
        }
    }

    /// Add a chunk
    #[cfg(feature = "std")]
    pub fn add_chunk(&mut self, cid: String, sequence: u32) {
        self.chunks.push(FileChunk { cid, sequence });
    }

    /// Serialize to protobuf bytes
    #[cfg(feature = "std")]
    pub fn to_bytes(&self) -> Result<Vec<u8>, FileSystemError> {
        let mut buf = Vec::new();
        self.encode(&mut buf)
            .map_err(|_| FileSystemError::SerializationError)?;
        Ok(buf)
    }

    /// Deserialize from protobuf bytes
    #[cfg(feature = "std")]
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, FileSystemError> {
        Self::decode(bytes).map_err(|_| FileSystemError::DeserializationError)
    }

    /// Compute the CID (blake2-256 hash) of this file manifest
    #[cfg(feature = "std")]
    pub fn compute_cid(&self) -> Result<Cid, FileSystemError> {
        let bytes = self.to_bytes()?;
        Ok(compute_cid(&bytes))
    }
}

impl DirectoryEntry {
    /// Create a new directory entry
    #[cfg(feature = "std")]
    pub fn new(name: String, entry_type: EntryType, cid: String, size: u64, mtime: u64) -> Self {
        Self {
            name,
            r#type: entry_type as i32,
            cid,
            size,
            mtime,
        }
    }

    /// Check if this entry is a directory
    pub fn is_directory(&self) -> bool {
        self.r#type == EntryType::Directory as i32
    }

    /// Check if this entry is a file
    pub fn is_file(&self) -> bool {
        self.r#type == EntryType::File as i32
    }

    /// Get the entry type
    pub fn entry_type(&self) -> EntryType {
        match self.r#type {
            0 => EntryType::File,
            1 => EntryType::Directory,
            _ => EntryType::File, // Default to file
        }
    }
}

/// Compute blake2-256 CID for data
pub fn compute_cid(data: &[u8]) -> Cid {
    use blake2::{Blake2b512, Digest};
    let mut hasher = Blake2b512::new();
    hasher.update(data);
    let result = hasher.finalize();
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&result[..32]);
    H256::from(hash)
}

/// Convert CID to hex string (for protobuf storage)
pub fn cid_to_string(cid: &Cid) -> String {
    alloc::format!("0x{}", hex::encode(cid.as_bytes()))
}

/// Parse hex string to CID
pub fn string_to_cid(s: &str) -> Result<Cid, FileSystemError> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    let bytes = hex::decode(s).map_err(|_| FileSystemError::InvalidCid)?;
    if bytes.len() != 32 {
        return Err(FileSystemError::InvalidCid);
    }
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&bytes);
    Ok(H256::from(hash))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_directory_node_serialization() {
        let mut dir = DirectoryNode::new_empty("drive_123".to_string());
        dir.add_child(DirectoryEntry::new(
            "file1.txt".to_string(),
            EntryType::File,
            "0xabc123".to_string(),
            1024,
            1234567890,
        ));

        let bytes = dir.to_bytes().unwrap();
        let decoded = DirectoryNode::from_bytes(&bytes).unwrap();

        assert_eq!(decoded.drive_id, "drive_123");
        assert_eq!(decoded.children.len(), 1);
        assert_eq!(decoded.children[0].name, "file1.txt");
    }

    #[test]
    fn test_file_manifest_serialization() {
        let mut manifest =
            FileManifest::new("drive_123".to_string(), "text/plain".to_string(), 2048);
        manifest.add_chunk("0xchunk1".to_string(), 0);
        manifest.add_chunk("0xchunk2".to_string(), 1);

        let bytes = manifest.to_bytes().unwrap();
        let decoded = FileManifest::from_bytes(&bytes).unwrap();

        assert_eq!(decoded.drive_id, "drive_123");
        assert_eq!(decoded.total_size, 2048);
        assert_eq!(decoded.chunks.len(), 2);
    }

    #[test]
    fn test_compute_cid() {
        let data = b"hello world";
        let cid = compute_cid(data);
        assert_eq!(cid.as_bytes().len(), 32);
    }

    #[test]
    fn test_cid_string_conversion() {
        let cid = compute_cid(b"test");
        let s = cid_to_string(&cid);
        let decoded = string_to_cid(&s).unwrap();
        assert_eq!(cid, decoded);
    }

    #[test]
    fn test_directory_operations() {
        let mut dir = DirectoryNode::new_empty("drive_1".to_string());

        // Add children
        dir.add_child(DirectoryEntry::new(
            "folder1".to_string(),
            EntryType::Directory,
            "0x123".to_string(),
            0,
            1000,
        ));
        dir.add_child(DirectoryEntry::new(
            "file1.txt".to_string(),
            EntryType::File,
            "0x456".to_string(),
            1024,
            2000,
        ));

        // Find child
        let found = dir.find_child("folder1");
        assert!(found.is_some());
        assert!(found.unwrap().is_directory());

        // Remove child
        let removed = dir.remove_child("file1.txt");
        assert!(removed.is_some());
        assert_eq!(dir.children.len(), 1);
    }
}
