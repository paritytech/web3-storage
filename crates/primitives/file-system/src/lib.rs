// SPDX-License-Identifier: Apache-2.0

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
//! - **DirectoryNode**: A directory containing references to children
//! - **FileManifest**: Metadata about a file and its chunks
//! - **CID**: Content Identifier (Merkle leaf hash)

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

#[cfg(feature = "std")]
extern crate std;

use alloc::{string::String, vec::Vec};
use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use scale_info::TypeInfo;
use sp_core::H256;
use sp_runtime::{traits::Get, BoundedVec};
use storage_primitives::BucketId;

// ============================================================================
// SCALE-encoded types (no_std compatible, used on-chain)
// ============================================================================

/// Content Identifier (Merkle leaf hash)
pub type Cid = H256;

/// Entry type enumeration (SCALE-encoded, no_std compatible)
#[derive(
    Clone,
    Copy,
    Encode,
    Decode,
    Default,
    DecodeWithMemTracking,
    Eq,
    PartialEq,
    Debug,
    TypeInfo,
    MaxEncodedLen,
)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub enum EntryType {
    /// A file entry
    #[codec(index = 0)]
    #[default]
    File,
    /// A directory entry
    #[codec(index = 1)]
    Directory,
}

/// Maximum length for entry names (256 bytes)
pub struct MaxEntryNameLength;
impl Get<u32> for MaxEntryNameLength {
    fn get() -> u32 {
        256
    }
}

/// Maximum number of children in a directory (1024)
pub struct MaxDirectoryChildren;
impl Get<u32> for MaxDirectoryChildren {
    fn get() -> u32 {
        1024
    }
}

/// Maximum number of metadata entries (64)
pub struct MaxMetadataEntries;
impl Get<u32> for MaxMetadataEntries {
    fn get() -> u32 {
        64
    }
}

/// Maximum length for metadata keys (64 bytes)
pub struct MaxMetadataKeyLength;
impl Get<u32> for MaxMetadataKeyLength {
    fn get() -> u32 {
        64
    }
}

/// Maximum length for metadata values (256 bytes)
pub struct MaxMetadataValueLength;
impl Get<u32> for MaxMetadataValueLength {
    fn get() -> u32 {
        256
    }
}

/// Maximum number of chunks in a file (65536)
pub struct MaxFileChunks;
impl Get<u32> for MaxFileChunks {
    fn get() -> u32 {
        65536
    }
}

/// Maximum length for MIME type strings (128 bytes)
pub struct MaxMimeTypeLength;
impl Get<u32> for MaxMimeTypeLength {
    fn get() -> u32 {
        128
    }
}

/// Maximum length for encryption params (512 bytes)
pub struct MaxEncryptionParamsLength;
impl Get<u32> for MaxEncryptionParamsLength {
    fn get() -> u32 {
        512
    }
}

/// A single entry in a directory (SCALE-encoded, no_std compatible)
#[derive(
    Clone, Encode, Decode, DecodeWithMemTracking, Eq, PartialEq, Debug, TypeInfo, MaxEncodedLen,
)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub struct DirectoryEntry {
    /// Human-readable name
    pub name: BoundedVec<u8, MaxEntryNameLength>,
    /// File or Directory
    pub entry_type: EntryType,
    /// Content ID (Merkle leaf hash)
    pub cid: Cid,
    /// Size in bytes
    pub size: u64,
    /// Modification timestamp (Unix timestamp)
    pub mtime: u64,
}

impl DirectoryEntry {
    /// Create a new directory entry
    #[cfg(feature = "std")]
    pub fn new(name: String, entry_type: EntryType, cid: Cid, size: u64, mtime: u64) -> Self {
        Self {
            name: BoundedVec::try_from(name.into_bytes()).unwrap_or_default(),
            entry_type,
            cid,
            size,
            mtime,
        }
    }

    /// Get the name as a string (lossy conversion)
    pub fn name_str(&self) -> String {
        String::from_utf8_lossy(&self.name).into_owned()
    }

    /// Check if this entry is a directory
    pub fn is_directory(&self) -> bool {
        self.entry_type == EntryType::Directory
    }

    /// Check if this entry is a file
    pub fn is_file(&self) -> bool {
        self.entry_type == EntryType::File
    }
}

/// Metadata key-value pair
#[derive(
    Clone, Encode, Decode, DecodeWithMemTracking, Eq, PartialEq, Debug, TypeInfo, MaxEncodedLen,
)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub struct MetadataEntry {
    pub key: BoundedVec<u8, MaxMetadataKeyLength>,
    pub value: BoundedVec<u8, MaxMetadataValueLength>,
}

/// Directory node containing child references (SCALE-encoded, no_std compatible)
#[derive(
    Clone, Encode, Decode, DecodeWithMemTracking, Eq, PartialEq, Debug, TypeInfo, MaxEncodedLen,
)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub struct DirectoryNode {
    /// Layer 0 bucket id of the drive
    pub drive_id: BucketId,
    /// Child entries
    pub children: BoundedVec<DirectoryEntry, MaxDirectoryChildren>,
    /// Custom metadata (tags, colors, etc.)
    pub metadata: BoundedVec<MetadataEntry, MaxMetadataEntries>,
}

impl DirectoryNode {
    /// Create a new empty directory
    pub fn new_empty(drive_id: BucketId) -> Self {
        Self {
            drive_id,
            children: BoundedVec::default(),
            metadata: BoundedVec::default(),
        }
    }

    /// Add a child entry
    pub fn add_child(&mut self, entry: DirectoryEntry) -> Result<(), DirectoryEntry> {
        self.children.try_push(entry)
    }

    /// Find a child by name
    pub fn find_child(&self, name: &str) -> Option<&DirectoryEntry> {
        self.children
            .iter()
            .find(|e| e.name.as_slice() == name.as_bytes())
    }

    /// Find a child by name (mutable)
    pub fn find_child_mut(&mut self, name: &str) -> Option<&mut DirectoryEntry> {
        self.children
            .iter_mut()
            .find(|e| e.name.as_slice() == name.as_bytes())
    }

    /// Remove a child by name
    pub fn remove_child(&mut self, name: &str) -> Option<DirectoryEntry> {
        if let Some(pos) = self
            .children
            .iter()
            .position(|e| e.name.as_slice() == name.as_bytes())
        {
            Some(self.children.remove(pos))
        } else {
            None
        }
    }

    /// Serialize to SCALE bytes
    pub fn to_scale_bytes(&self) -> Vec<u8> {
        self.encode()
    }

    /// Deserialize from SCALE bytes
    pub fn from_scale_bytes(bytes: &[u8]) -> Result<Self, codec::Error> {
        Self::decode(&mut &bytes[..])
    }

    /// Compute the CID (Merkle leaf hash) of this directory node
    pub fn compute_cid(&self) -> Cid {
        compute_cid(&self.to_scale_bytes())
    }
}

/// A single chunk reference in a file (SCALE-encoded, no_std compatible)
#[derive(
    Clone, Encode, Decode, DecodeWithMemTracking, Eq, PartialEq, Debug, TypeInfo, MaxEncodedLen,
)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub struct FileChunk {
    /// Chunk CID (Merkle leaf hash)
    pub cid: Cid,
    /// Position in the file (0-indexed)
    pub sequence: u32,
}

/// File manifest tracking how to reassemble a file from chunks (SCALE-encoded, no_std compatible)
#[derive(
    Clone, Encode, Decode, DecodeWithMemTracking, Eq, PartialEq, Debug, TypeInfo, MaxEncodedLen,
)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub struct FileManifest {
    /// Layer 0 bucket id of the drive
    pub drive_id: BucketId,
    /// MIME type (e.g., "image/png")
    pub mime_type: BoundedVec<u8, MaxMimeTypeLength>,
    /// Total file size in bytes
    pub total_size: u64,
    /// Ordered list of chunks
    pub chunks: BoundedVec<FileChunk, MaxFileChunks>,
    /// Encryption parameters (optional, for W3ACL)
    pub encryption_params: BoundedVec<u8, MaxEncryptionParamsLength>,
}

impl FileManifest {
    /// Create a new file manifest
    #[cfg(feature = "std")]
    pub fn new(drive_id: BucketId, mime_type: String, total_size: u64) -> Self {
        Self {
            drive_id,
            mime_type: BoundedVec::try_from(mime_type.into_bytes()).unwrap_or_default(),
            total_size,
            chunks: BoundedVec::default(),
            encryption_params: BoundedVec::default(),
        }
    }

    /// Add a chunk
    pub fn add_chunk(&mut self, cid: Cid, sequence: u32) -> Result<(), FileChunk> {
        self.chunks.try_push(FileChunk { cid, sequence })
    }

    /// Get the MIME type as a string
    pub fn mime_type_str(&self) -> String {
        String::from_utf8_lossy(&self.mime_type).into_owned()
    }

    /// Serialize to SCALE bytes
    pub fn to_scale_bytes(&self) -> Vec<u8> {
        self.encode()
    }

    /// Deserialize from SCALE bytes
    pub fn from_scale_bytes(bytes: &[u8]) -> Result<Self, codec::Error> {
        Self::decode(&mut &bytes[..])
    }

    /// Compute the CID (Merkle leaf hash) of this file manifest
    pub fn compute_cid(&self) -> Cid {
        compute_cid(&self.to_scale_bytes())
    }
}

// ============================================================================
// Error types
// ============================================================================

/// Error types for file system operations
#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode, TypeInfo)]
#[cfg_attr(feature = "std", derive(thiserror::Error))]
pub enum FileSystemError {
    #[cfg_attr(feature = "std", error("Entry not found: {0}"))]
    EntryNotFound(String),

    #[cfg_attr(feature = "std", error("Invalid path"))]
    InvalidPath,

    #[cfg_attr(feature = "std", error("Not a directory"))]
    NotADirectory,

    #[cfg_attr(feature = "std", error("Not a file"))]
    NotAFile,
}

// ============================================================================
// Utility functions
// ============================================================================

/// Compute the CID of data: its Merkle leaf hash, which is the data root of a
/// single-chunk blob.
pub fn compute_cid(data: &[u8]) -> Cid {
    storage_primitives::hash_leaf(data)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_directory_encoding() {
        let dir = DirectoryNode::new_empty(2);
        let bytes = dir.to_scale_bytes();
        println!("Empty DirectoryNode for drive_id=2:");
        println!("  Length: {}", bytes.len());
        println!("  Hex: {}", array_bytes::bytes2hex("", &bytes));
        let cid = compute_cid(&bytes);
        println!("  CID: {cid:#x}");

        // Also test roundtrip
        let decoded = DirectoryNode::from_scale_bytes(&bytes).unwrap();
        assert_eq!(decoded.drive_id, 2);
        assert!(decoded.children.is_empty());
    }

    #[test]
    fn test_directory_node_scale_serialization() {
        let mut dir = DirectoryNode::new_empty(123);
        let entry = DirectoryEntry {
            name: BoundedVec::try_from(b"file1.txt".to_vec()).unwrap(),
            entry_type: EntryType::File,
            cid: compute_cid(b"test"),
            size: 1024,
            mtime: 1234567890,
        };
        dir.add_child(entry).unwrap();

        let bytes = dir.to_scale_bytes();
        let decoded = DirectoryNode::from_scale_bytes(&bytes).unwrap();

        assert_eq!(decoded.drive_id, 123);
        assert_eq!(decoded.children.len(), 1);
        assert_eq!(decoded.children[0].name_str(), "file1.txt");
    }

    #[test]
    fn test_file_manifest_scale_serialization() {
        let mut manifest = FileManifest {
            drive_id: 123,
            mime_type: BoundedVec::try_from(b"text/plain".to_vec()).unwrap(),
            total_size: 2048,
            chunks: BoundedVec::default(),
            encryption_params: BoundedVec::default(),
        };
        manifest.add_chunk(compute_cid(b"chunk1"), 0).unwrap();
        manifest.add_chunk(compute_cid(b"chunk2"), 1).unwrap();

        let bytes = manifest.to_scale_bytes();
        let decoded = FileManifest::from_scale_bytes(&bytes).unwrap();

        assert_eq!(decoded.drive_id, 123);
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
    fn test_directory_operations() {
        let mut dir = DirectoryNode::new_empty(1);

        // Add children
        let folder = DirectoryEntry {
            name: BoundedVec::try_from(b"folder1".to_vec()).unwrap(),
            entry_type: EntryType::Directory,
            cid: compute_cid(b"folder"),
            size: 0,
            mtime: 1000,
        };
        let file = DirectoryEntry {
            name: BoundedVec::try_from(b"file1.txt".to_vec()).unwrap(),
            entry_type: EntryType::File,
            cid: compute_cid(b"file"),
            size: 1024,
            mtime: 2000,
        };
        dir.add_child(folder).unwrap();
        dir.add_child(file).unwrap();

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
