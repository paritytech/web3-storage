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
//!
//! # Type System
//!
//! This crate provides two sets of types:
//! - **SCALE types** (always available): Used for on-chain storage, `no_std` compatible
//! - **Proto types** (std only): Used for off-chain serialization via protobuf

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

#[cfg(feature = "std")]
extern crate std;

use alloc::{string::String, vec::Vec};
use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use scale_info::TypeInfo;
use sp_core::H256;
use sp_runtime::{traits::Get, BoundedVec, RuntimeDebug};

// ============================================================================
// Protobuf types (std only)
// ============================================================================

#[cfg(feature = "std")]
use prost::Message;

/// Protobuf-generated types for off-chain serialization (std only)
#[cfg(feature = "std")]
pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/filesystem.rs"));
}

// ============================================================================
// SCALE-encoded types (no_std compatible, used on-chain)
// ============================================================================

/// Drive identifier (unique ID for each drive)
pub type DriveId = u64;

/// Agreement identifier from Layer 0
pub type AgreementId = u64;

/// Content Identifier (blake2-256 hash)
pub type Cid = H256;

/// Entry type enumeration (SCALE-encoded, no_std compatible)
#[derive(Clone, Copy, Encode, Decode, DecodeWithMemTracking, Eq, PartialEq, RuntimeDebug, TypeInfo, MaxEncodedLen)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub enum EntryType {
    /// A file entry
    #[codec(index = 0)]
    File,
    /// A directory entry
    #[codec(index = 1)]
    Directory,
}

impl Default for EntryType {
    fn default() -> Self {
        Self::File
    }
}

/// Maximum length for entry names (256 bytes)
pub struct MaxEntryNameLength;
impl Get<u32> for MaxEntryNameLength {
    fn get() -> u32 {
        256
    }
}

/// Maximum length for CID strings (66 bytes for "0x" + 64 hex chars)
pub struct MaxCidStringLength;
impl Get<u32> for MaxCidStringLength {
    fn get() -> u32 {
        66
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
#[derive(Clone, Encode, Decode, DecodeWithMemTracking, Eq, PartialEq, RuntimeDebug, TypeInfo, MaxEncodedLen)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub struct DirectoryEntry {
    /// Human-readable name
    pub name: BoundedVec<u8, MaxEntryNameLength>,
    /// File or Directory
    pub entry_type: EntryType,
    /// Content ID (blake2-256 hash)
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
#[derive(Clone, Encode, Decode, DecodeWithMemTracking, Eq, PartialEq, RuntimeDebug, TypeInfo, MaxEncodedLen)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub struct MetadataEntry {
    pub key: BoundedVec<u8, MaxMetadataKeyLength>,
    pub value: BoundedVec<u8, MaxMetadataValueLength>,
}

/// Directory node containing child references (SCALE-encoded, no_std compatible)
#[derive(Clone, Encode, Decode, DecodeWithMemTracking, Eq, PartialEq, RuntimeDebug, TypeInfo, MaxEncodedLen)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub struct DirectoryNode {
    /// Drive ID this directory belongs to
    pub drive_id: DriveId,
    /// Child entries
    pub children: BoundedVec<DirectoryEntry, MaxDirectoryChildren>,
    /// Custom metadata (tags, colors, etc.)
    pub metadata: BoundedVec<MetadataEntry, MaxMetadataEntries>,
}

impl DirectoryNode {
    /// Create a new empty directory
    pub fn new_empty(drive_id: DriveId) -> Self {
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
        self.children.iter().find(|e| e.name_str() == name)
    }

    /// Find a child by name (mutable)
    pub fn find_child_mut(&mut self, name: &str) -> Option<&mut DirectoryEntry> {
        self.children.iter_mut().find(|e| e.name_str() == name)
    }

    /// Remove a child by name
    pub fn remove_child(&mut self, name: &str) -> Option<DirectoryEntry> {
        if let Some(pos) = self.children.iter().position(|e| e.name_str() == name) {
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

    /// Compute the CID (blake2-256 hash) of this directory node
    pub fn compute_cid(&self) -> Cid {
        compute_cid(&self.to_scale_bytes())
    }
}

/// A single chunk reference in a file (SCALE-encoded, no_std compatible)
#[derive(Clone, Encode, Decode, DecodeWithMemTracking, Eq, PartialEq, RuntimeDebug, TypeInfo, MaxEncodedLen)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub struct FileChunk {
    /// Chunk CID (blake2-256 hash)
    pub cid: Cid,
    /// Position in the file (0-indexed)
    pub sequence: u32,
}

/// File manifest tracking how to reassemble a file from chunks (SCALE-encoded, no_std compatible)
#[derive(Clone, Encode, Decode, DecodeWithMemTracking, Eq, PartialEq, RuntimeDebug, TypeInfo, MaxEncodedLen)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub struct FileManifest {
    /// Drive ID this file belongs to
    pub drive_id: DriveId,
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
    pub fn new(drive_id: DriveId, mime_type: String, total_size: u64) -> Self {
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

    /// Compute the CID (blake2-256 hash) of this file manifest
    pub fn compute_cid(&self) -> Cid {
        compute_cid(&self.to_scale_bytes())
    }
}

// ============================================================================
// Conversion between SCALE and Proto types (std only)
// ============================================================================

#[cfg(feature = "std")]
impl From<EntryType> for proto::EntryType {
    fn from(entry_type: EntryType) -> Self {
        match entry_type {
            EntryType::File => proto::EntryType::File,
            EntryType::Directory => proto::EntryType::Directory,
        }
    }
}

#[cfg(feature = "std")]
impl From<proto::EntryType> for EntryType {
    fn from(entry_type: proto::EntryType) -> Self {
        match entry_type {
            proto::EntryType::File => EntryType::File,
            proto::EntryType::Directory => EntryType::Directory,
        }
    }
}

#[cfg(feature = "std")]
impl From<&DirectoryEntry> for proto::DirectoryEntry {
    fn from(entry: &DirectoryEntry) -> Self {
        Self {
            name: entry.name_str(),
            r#type: proto::EntryType::from(entry.entry_type) as i32,
            cid: cid_to_string(&entry.cid),
            size: entry.size,
            mtime: entry.mtime,
        }
    }
}

#[cfg(feature = "std")]
impl TryFrom<&proto::DirectoryEntry> for DirectoryEntry {
    type Error = FileSystemError;

    fn try_from(entry: &proto::DirectoryEntry) -> Result<Self, Self::Error> {
        let entry_type = match entry.r#type {
            0 => EntryType::File,
            1 => EntryType::Directory,
            _ => EntryType::File,
        };
        Ok(Self {
            name: BoundedVec::try_from(entry.name.clone().into_bytes())
                .map_err(|_| FileSystemError::InvalidPath)?,
            entry_type,
            cid: string_to_cid(&entry.cid)?,
            size: entry.size,
            mtime: entry.mtime,
        })
    }
}

#[cfg(feature = "std")]
impl From<&DirectoryNode> for proto::DirectoryNode {
    fn from(node: &DirectoryNode) -> Self {
        Self {
            drive_id: node.drive_id.to_string(),
            children: node.children.iter().map(proto::DirectoryEntry::from).collect(),
            metadata: node
                .metadata
                .iter()
                .map(|m| {
                    (
                        String::from_utf8_lossy(&m.key).into_owned(),
                        String::from_utf8_lossy(&m.value).into_owned(),
                    )
                })
                .collect(),
        }
    }
}

#[cfg(feature = "std")]
impl TryFrom<&proto::DirectoryNode> for DirectoryNode {
    type Error = FileSystemError;

    fn try_from(node: &proto::DirectoryNode) -> Result<Self, Self::Error> {
        let drive_id: DriveId = node.drive_id.parse().map_err(|_| FileSystemError::InvalidPath)?;
        let children: Result<Vec<DirectoryEntry>, _> =
            node.children.iter().map(DirectoryEntry::try_from).collect();
        let metadata: Result<Vec<MetadataEntry>, _> = node
            .metadata
            .iter()
            .map(|(k, v)| {
                Ok(MetadataEntry {
                    key: BoundedVec::try_from(k.clone().into_bytes())
                        .map_err(|_| FileSystemError::InvalidPath)?,
                    value: BoundedVec::try_from(v.clone().into_bytes())
                        .map_err(|_| FileSystemError::InvalidPath)?,
                })
            })
            .collect();

        Ok(Self {
            drive_id,
            children: BoundedVec::try_from(children?).map_err(|_| FileSystemError::InvalidPath)?,
            metadata: BoundedVec::try_from(metadata?).map_err(|_| FileSystemError::InvalidPath)?,
        })
    }
}

#[cfg(feature = "std")]
impl From<&FileManifest> for proto::FileManifest {
    fn from(manifest: &FileManifest) -> Self {
        Self {
            drive_id: manifest.drive_id.to_string(),
            mime_type: manifest.mime_type_str(),
            total_size: manifest.total_size,
            chunks: manifest
                .chunks
                .iter()
                .map(|c| proto::FileChunk {
                    cid: cid_to_string(&c.cid),
                    sequence: c.sequence,
                })
                .collect(),
            encryption_params: String::from_utf8_lossy(&manifest.encryption_params).into_owned(),
        }
    }
}

#[cfg(feature = "std")]
impl TryFrom<&proto::FileManifest> for FileManifest {
    type Error = FileSystemError;

    fn try_from(manifest: &proto::FileManifest) -> Result<Self, Self::Error> {
        let drive_id: DriveId = manifest
            .drive_id
            .parse()
            .map_err(|_| FileSystemError::InvalidPath)?;
        let chunks: Result<Vec<FileChunk>, _> = manifest
            .chunks
            .iter()
            .map(|c| {
                Ok(FileChunk {
                    cid: string_to_cid(&c.cid)?,
                    sequence: c.sequence,
                })
            })
            .collect();

        Ok(Self {
            drive_id,
            mime_type: BoundedVec::try_from(manifest.mime_type.clone().into_bytes())
                .map_err(|_| FileSystemError::InvalidPath)?,
            total_size: manifest.total_size,
            chunks: BoundedVec::try_from(chunks?).map_err(|_| FileSystemError::InvalidPath)?,
            encryption_params: BoundedVec::try_from(manifest.encryption_params.clone().into_bytes())
                .map_err(|_| FileSystemError::InvalidPath)?,
        })
    }
}

// ============================================================================
// Protobuf serialization helpers (std only)
// ============================================================================

#[cfg(feature = "std")]
impl DirectoryNode {
    /// Serialize to protobuf bytes
    pub fn to_proto_bytes(&self) -> Result<Vec<u8>, FileSystemError> {
        let proto_node = proto::DirectoryNode::from(self);
        let mut buf = Vec::new();
        proto_node
            .encode(&mut buf)
            .map_err(|_| FileSystemError::SerializationError)?;
        Ok(buf)
    }

    /// Deserialize from protobuf bytes
    pub fn from_proto_bytes(bytes: &[u8]) -> Result<Self, FileSystemError> {
        let proto_node =
            proto::DirectoryNode::decode(bytes).map_err(|_| FileSystemError::DeserializationError)?;
        Self::try_from(&proto_node)
    }
}

#[cfg(feature = "std")]
impl FileManifest {
    /// Serialize to protobuf bytes
    pub fn to_proto_bytes(&self) -> Result<Vec<u8>, FileSystemError> {
        let proto_manifest = proto::FileManifest::from(self);
        let mut buf = Vec::new();
        proto_manifest
            .encode(&mut buf)
            .map_err(|_| FileSystemError::SerializationError)?;
        Ok(buf)
    }

    /// Deserialize from protobuf bytes
    pub fn from_proto_bytes(bytes: &[u8]) -> Result<Self, FileSystemError> {
        let proto_manifest =
            proto::FileManifest::decode(bytes).map_err(|_| FileSystemError::DeserializationError)?;
        Self::try_from(&proto_manifest)
    }
}

// ============================================================================
// Error types
// ============================================================================

/// Error types for file system operations
#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode, TypeInfo)]
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

// ============================================================================
// Commit strategy and drive configuration
// ============================================================================

/// Strategy for committing changes to the on-chain root CID
#[derive(Clone, Copy, Encode, Decode, DecodeWithMemTracking, Eq, PartialEq, RuntimeDebug, TypeInfo, MaxEncodedLen)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub enum CommitStrategy {
    /// Commit every change immediately (expensive, real-time)
    #[codec(index = 0)]
    Immediate,
    /// Commit changes in batches after N blocks
    #[codec(index = 1)]
    Batched { interval: u32 },
    /// User manually triggers commits
    #[codec(index = 2)]
    Manual,
}

impl Default for CommitStrategy {
    fn default() -> Self {
        // Default to batched commits every 100 blocks (~10 minutes)
        Self::Batched { interval: 100 }
    }
}

/// Configuration for creating a drive with storage
#[cfg(feature = "std")]
#[derive(Clone, Debug)]
pub struct DriveConfig<AccountId> {
    /// Total storage size in bytes
    pub storage_size: u64,
    /// Budget for storage agreements (total across all providers)
    pub budget: u128,
    /// Number of storage providers (1 primary + N-1 replicas)
    pub num_providers: u8,
    /// Preferred providers (optional)
    pub preferred_providers: Vec<AccountId>,
    /// When to commit changes on-chain
    pub commit_strategy: CommitStrategy,
}

#[cfg(feature = "std")]
impl<AccountId> Default for DriveConfig<AccountId> {
    fn default() -> Self {
        Self {
            storage_size: 10 * 1024 * 1024 * 1024, // 10 GB
            budget: 100_000_000_000_000,           // 100 tokens (assuming 12 decimals)
            num_providers: 3,                      // 1 primary + 2 replicas
            preferred_providers: Vec::new(),
            commit_strategy: CommitStrategy::default(),
        }
    }
}

/// Drive information stored on-chain (user's virtual drive)
#[derive(Clone, Encode, Decode, Eq, PartialEq, RuntimeDebug, TypeInfo, MaxEncodedLen)]
#[scale_info(skip_type_params(MaxNameLength, Balance))]
#[codec(mel_bound())]
pub struct DriveInfo<
    AccountId: Encode + Decode + MaxEncodedLen,
    BlockNumber: Encode + Decode + MaxEncodedLen,
    MaxNameLength: Get<u32>,
    Balance: Encode + Decode + MaxEncodedLen,
> {
    /// Owner of the drive
    pub owner: AccountId,
    /// Layer 0 bucket ID this drive uses
    pub bucket_id: u64,
    /// Current committed root CID (on-chain, visible to all)
    pub root_cid: Cid,
    /// Pending root CID (not yet committed, only in local state)
    /// This is stored as an option - None means no pending changes
    pub pending_root_cid: Option<Cid>,
    /// Strategy for committing changes
    pub commit_strategy: CommitStrategy,
    /// Block number when drive was created
    pub created_at: BlockNumber,
    /// Block number when root_cid was last committed
    pub last_committed_at: BlockNumber,
    /// Optional human-readable name (bounded)
    pub name: Option<BoundedVec<u8, MaxNameLength>>,
    /// Maximum storage capacity in bytes
    pub max_capacity: u64,
    /// Storage period in blocks
    pub storage_period: BlockNumber,
    /// Expiry block number (created_at + storage_period)
    pub expires_at: BlockNumber,
    /// Payment tokens for storage
    pub payment: Balance,
}

// ============================================================================
// Utility functions
// ============================================================================

/// Compute blake2-256 CID for data
pub fn compute_cid(data: &[u8]) -> Cid {
    sp_core::hashing::blake2_256(data).into()
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

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

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
        manifest
            .add_chunk(compute_cid(b"chunk1"), 0)
            .unwrap();
        manifest
            .add_chunk(compute_cid(b"chunk2"), 1)
            .unwrap();

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
    fn test_cid_string_conversion() {
        let cid = compute_cid(b"test");
        let s = cid_to_string(&cid);
        let decoded = string_to_cid(&s).unwrap();
        assert_eq!(cid, decoded);
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

    #[cfg(feature = "std")]
    #[test]
    fn test_proto_conversion() {
        let mut dir = DirectoryNode::new_empty(456);
        let entry = DirectoryEntry {
            name: BoundedVec::try_from(b"test.txt".to_vec()).unwrap(),
            entry_type: EntryType::File,
            cid: compute_cid(b"content"),
            size: 512,
            mtime: 1000000,
        };
        dir.add_child(entry).unwrap();

        // Convert to proto and back
        let proto_node = proto::DirectoryNode::from(&dir);
        let converted_back = DirectoryNode::try_from(&proto_node).unwrap();

        assert_eq!(dir.drive_id, converted_back.drive_id);
        assert_eq!(dir.children.len(), converted_back.children.len());
        assert_eq!(
            dir.children[0].name_str(),
            converted_back.children[0].name_str()
        );
    }

    #[cfg(feature = "std")]
    #[test]
    fn test_proto_bytes_serialization() {
        let mut dir = DirectoryNode::new_empty(789);
        let entry = DirectoryEntry {
            name: BoundedVec::try_from(b"doc.pdf".to_vec()).unwrap(),
            entry_type: EntryType::File,
            cid: compute_cid(b"pdf_content"),
            size: 4096,
            mtime: 2000000,
        };
        dir.add_child(entry).unwrap();

        // Serialize to proto bytes and back
        let proto_bytes = dir.to_proto_bytes().unwrap();
        let decoded = DirectoryNode::from_proto_bytes(&proto_bytes).unwrap();

        assert_eq!(dir.drive_id, decoded.drive_id);
        assert_eq!(dir.children.len(), decoded.children.len());
    }
}
