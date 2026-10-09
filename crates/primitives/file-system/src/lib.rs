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
//! - **CID**: Content Identifier of a blob: its Layer 0 `data_root`
//!   (see [`compute_cid`])
//!
//! The file system clients store every directory and manifest as a SCALE
//! encoded blob in the drive's bucket. The root directory is the data root of
//! the bucket's last MMR leaf.
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
use sp_runtime::{traits::Get, BoundedVec};

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

/// Content Identifier: the Layer 0 `data_root` of a blob (see [`compute_cid`]).
pub type Cid = H256;

/// Content type stored when the uploader gives none.
pub const DEFAULT_MIME_TYPE: &str = "application/octet-stream";

/// Chunk size of the [`compute_cid`] rule, in bytes (256 KiB).
pub const CHUNK_SIZE: usize = storage_primitives::DEFAULT_CHUNK_SIZE as usize;

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

/// A single entry in a directory (SCALE-encoded, no_std compatible).
///
/// `name` is 1..=256 bytes of UTF-8, contains no `/`, and is not `.` or `..`
/// (see [`validate_entry_name`]). For a file, `cid` is the CID of its
/// [`FileManifest`] and `size` the content size; for a directory, `cid` is
/// the CID of its [`DirectoryNode`] and `size` is 0.
#[derive(
    Clone, Encode, Decode, DecodeWithMemTracking, Eq, PartialEq, Debug, TypeInfo, MaxEncodedLen,
)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub struct DirectoryEntry {
    /// Human-readable name
    pub name: BoundedVec<u8, MaxEntryNameLength>,
    /// File or Directory
    pub entry_type: EntryType,
    /// CID of the file's manifest or the directory's node
    pub cid: Cid,
    /// Content size in bytes for a file, 0 for a directory
    pub size: u64,
    /// Modification timestamp (Unix timestamp)
    pub mtime: u64,
}

impl DirectoryEntry {
    /// Create a directory entry. Fails with
    /// [`FileSystemError::InvalidEntryName`] if `name` breaks the rules of
    /// [`validate_entry_name`].
    pub fn try_new(
        name: &str,
        entry_type: EntryType,
        cid: Cid,
        size: u64,
        mtime: u64,
    ) -> Result<Self, FileSystemError> {
        validate_entry_name(name.as_bytes())?;
        Ok(Self {
            name: BoundedVec::try_from(name.as_bytes().to_vec())
                .map_err(|_| FileSystemError::InvalidEntryName)?,
            entry_type,
            cid,
            size,
            mtime,
        })
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

/// Metadata key-value pair: key up to 64 bytes, value up to 256 bytes.
#[derive(
    Clone, Encode, Decode, DecodeWithMemTracking, Eq, PartialEq, Debug, TypeInfo, MaxEncodedLen,
)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub struct MetadataEntry {
    /// Metadata key
    pub key: BoundedVec<u8, MaxMetadataKeyLength>,
    /// Metadata value
    pub value: BoundedVec<u8, MaxMetadataValueLength>,
}

impl MetadataEntry {
    /// Create an entry. Fails with [`FileSystemError::BoundsExceeded`] if the
    /// key is over 64 bytes or the value over 256 bytes.
    pub fn try_new(key: &[u8], value: &[u8]) -> Result<Self, FileSystemError> {
        Ok(Self {
            key: BoundedVec::try_from(key.to_vec()).map_err(|_| FileSystemError::BoundsExceeded)?,
            value: BoundedVec::try_from(value.to_vec())
                .map_err(|_| FileSystemError::BoundsExceeded)?,
        })
    }
}

/// Directory node containing child references (SCALE-encoded, no_std compatible).
///
/// `children` are sorted by name bytes, ascending, and names are unique.
/// `metadata` is reserved and empty. [`DirectoryNode::validate`] checks both.
#[derive(
    Clone, Encode, Decode, DecodeWithMemTracking, Eq, PartialEq, Debug, TypeInfo, MaxEncodedLen,
)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub struct DirectoryNode {
    /// Layer 0 bucket id of the drive
    pub drive_id: DriveId,
    /// Child entries, sorted by name bytes
    pub children: BoundedVec<DirectoryEntry, MaxDirectoryChildren>,
    /// Reserved, empty
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

    /// Insert `entry`, or replace the child with the same name. Keeps the
    /// children sorted. Fails with [`FileSystemError::BoundsExceeded`] if the
    /// directory already has 1024 children.
    pub fn upsert_child(&mut self, entry: DirectoryEntry) -> Result<(), FileSystemError> {
        match self.position(&entry.name) {
            Ok(pos) => {
                self.children[pos] = entry;
                Ok(())
            }
            Err(pos) => self
                .children
                .try_insert(pos, entry)
                .map_err(|_| FileSystemError::BoundsExceeded),
        }
    }

    /// Find a child by name
    pub fn find_child(&self, name: &str) -> Option<&DirectoryEntry> {
        self.position(name.as_bytes())
            .ok()
            .map(|pos| &self.children[pos])
    }

    /// Remove a child by name
    pub fn remove_child(&mut self, name: &str) -> Option<DirectoryEntry> {
        self.position(name.as_bytes())
            .ok()
            .map(|pos| self.children.remove(pos))
    }

    /// Binary search for `name` in the sorted children.
    fn position(&self, name: &[u8]) -> Result<usize, usize> {
        self.children
            .binary_search_by(|e| e.name.as_slice().cmp(name))
    }

    /// Check the rules SCALE decoding does not: valid, sorted, unique child
    /// names, size 0 on directory entries, and empty `metadata`.
    pub fn validate(&self) -> Result<(), FileSystemError> {
        if !self.metadata.is_empty() {
            return Err(FileSystemError::InvalidDirectory);
        }
        for entry in self.children.iter() {
            validate_entry_name(&entry.name)?;
            if entry.is_directory() && entry.size != 0 {
                return Err(FileSystemError::InvalidDirectory);
            }
        }
        if self
            .children
            .windows(2)
            .any(|pair| pair[0].name.as_slice() >= pair[1].name.as_slice())
        {
            return Err(FileSystemError::InvalidDirectory);
        }
        Ok(())
    }

    /// Serialize to SCALE bytes
    pub fn to_scale_bytes(&self) -> Vec<u8> {
        self.encode()
    }

    /// Deserialize from SCALE bytes
    pub fn from_scale_bytes(bytes: &[u8]) -> Result<Self, codec::Error> {
        Self::decode(&mut &bytes[..])
    }

    /// CID of the SCALE encoding (see [`compute_cid`])
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
    /// blake2-256 hash of the chunk
    pub cid: Cid,
    /// Position in the file (0-indexed)
    pub sequence: u32,
}

/// File manifest tracking how to reassemble a file from chunks (SCALE-encoded, no_std compatible).
///
/// `chunks` lists the hashes of the content's 256 KiB chunks in order, with
/// `sequence` equal to the index. Empty content has one chunk: the hash of
/// the empty chunk. The content's data root is [`FileManifest::content_root`].
#[derive(
    Clone, Encode, Decode, DecodeWithMemTracking, Eq, PartialEq, Debug, TypeInfo, MaxEncodedLen,
)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub struct FileManifest {
    /// Layer 0 bucket id of the drive
    pub drive_id: DriveId,
    /// MIME type (e.g., "image/png"); [`DEFAULT_MIME_TYPE`] if none was given
    pub mime_type: BoundedVec<u8, MaxMimeTypeLength>,
    /// Total file size in bytes
    pub total_size: u64,
    /// Ordered list of chunks
    pub chunks: BoundedVec<FileChunk, MaxFileChunks>,
    /// Encryption parameters (optional, for W3ACL); empty
    pub encryption_params: BoundedVec<u8, MaxEncryptionParamsLength>,
    /// User metadata (for example S3 `x-amz-meta-*` values)
    pub user_metadata: BoundedVec<MetadataEntry, MaxMetadataEntries>,
}

impl FileManifest {
    /// Build the manifest of `content`: its chunk hashes, size, `mime_type`
    /// ([`DEFAULT_MIME_TYPE`] if empty) and `user_metadata`. Fails with
    /// [`FileSystemError::BoundsExceeded`] if
    /// the MIME type is over 128 bytes, the content has more than 65536
    /// chunks, or there are more than 64 metadata entries.
    pub fn for_content(
        drive_id: DriveId,
        content: &[u8],
        mime_type: &str,
        user_metadata: Vec<MetadataEntry>,
    ) -> Result<Self, FileSystemError> {
        let chunks = chunk_hashes(content)
            .into_iter()
            .enumerate()
            .map(|(i, cid)| FileChunk {
                cid,
                sequence: i as u32,
            })
            .collect::<Vec<_>>();
        let mime_type = if mime_type.is_empty() {
            DEFAULT_MIME_TYPE
        } else {
            mime_type
        };
        Ok(Self {
            drive_id,
            mime_type: BoundedVec::try_from(mime_type.as_bytes().to_vec())
                .map_err(|_| FileSystemError::BoundsExceeded)?,
            total_size: content.len() as u64,
            chunks: BoundedVec::try_from(chunks).map_err(|_| FileSystemError::BoundsExceeded)?,
            encryption_params: BoundedVec::default(),
            user_metadata: BoundedVec::try_from(user_metadata)
                .map_err(|_| FileSystemError::BoundsExceeded)?,
        })
    }

    /// Data root of the content: the padded Merkle root over the chunk hashes.
    /// Equals [`compute_cid`] of the content.
    pub fn content_root(&self) -> Cid {
        let leaves: Vec<H256> = self.chunks.iter().map(|c| c.cid).collect();
        storage_primitives::padded_merkle_tree(&leaves).0
    }

    /// The MIME type as a string; [`DEFAULT_MIME_TYPE`] if the stored one is
    /// empty.
    pub fn mime_type_str(&self) -> String {
        if self.mime_type.is_empty() {
            return DEFAULT_MIME_TYPE.into();
        }
        String::from_utf8_lossy(&self.mime_type).into_owned()
    }

    /// Check the rules SCALE decoding does not: `chunks` has one entry per
    /// [`CHUNK_SIZE`] chunk of `total_size` bytes (one for empty content),
    /// and each chunk's `sequence` is its index.
    pub fn validate(&self) -> Result<(), FileSystemError> {
        let expected = self.total_size.div_ceil(CHUNK_SIZE as u64).max(1);
        if self.chunks.len() as u64 != expected {
            return Err(FileSystemError::InvalidManifest);
        }
        if self
            .chunks
            .iter()
            .enumerate()
            .any(|(i, chunk)| chunk.sequence as usize != i)
        {
            return Err(FileSystemError::InvalidManifest);
        }
        Ok(())
    }

    /// Serialize to SCALE bytes
    pub fn to_scale_bytes(&self) -> Vec<u8> {
        self.encode()
    }

    /// Deserialize from SCALE bytes
    pub fn from_scale_bytes(bytes: &[u8]) -> Result<Self, codec::Error> {
        Self::decode(&mut &bytes[..])
    }

    /// CID of the SCALE encoding (see [`compute_cid`])
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
            children: node
                .children
                .iter()
                .map(proto::DirectoryEntry::from)
                .collect(),
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
        let drive_id: DriveId = node
            .drive_id
            .parse()
            .map_err(|_| FileSystemError::InvalidPath)?;
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
            user_metadata: manifest
                .user_metadata
                .iter()
                .map(|m| proto::MetadataEntry {
                    key: String::from_utf8_lossy(&m.key).into_owned(),
                    value: String::from_utf8_lossy(&m.value).into_owned(),
                })
                .collect(),
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
        let user_metadata = manifest
            .user_metadata
            .iter()
            .map(|m| MetadataEntry::try_new(m.key.as_bytes(), m.value.as_bytes()))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self {
            drive_id,
            mime_type: BoundedVec::try_from(manifest.mime_type.clone().into_bytes())
                .map_err(|_| FileSystemError::InvalidPath)?,
            total_size: manifest.total_size,
            chunks: BoundedVec::try_from(chunks?).map_err(|_| FileSystemError::InvalidPath)?,
            encryption_params: BoundedVec::try_from(
                manifest.encryption_params.clone().into_bytes(),
            )
            .map_err(|_| FileSystemError::InvalidPath)?,
            user_metadata: BoundedVec::try_from(user_metadata)
                .map_err(|_| FileSystemError::BoundsExceeded)?,
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
        let proto_node = proto::DirectoryNode::decode(bytes)
            .map_err(|_| FileSystemError::DeserializationError)?;
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
        let proto_manifest = proto::FileManifest::decode(bytes)
            .map_err(|_| FileSystemError::DeserializationError)?;
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

    /// An entry name is empty, over 256 bytes, not UTF-8, contains `/`, or is
    /// `.` or `..`.
    #[cfg_attr(feature = "std", error("Invalid entry name"))]
    InvalidEntryName,

    /// A directory node has unsorted or duplicate child names, a directory
    /// entry with a non-zero size, or non-empty metadata.
    #[cfg_attr(feature = "std", error("Invalid directory node"))]
    InvalidDirectory,

    /// A file manifest's chunk count does not match its `total_size`, or a
    /// chunk's `sequence` is not its index.
    #[cfg_attr(feature = "std", error("Invalid file manifest"))]
    InvalidManifest,

    /// A value is over one of the format's size bounds.
    #[cfg_attr(feature = "std", error("Value exceeds a size bound"))]
    BoundsExceeded,
}

/// Drive information stored on-chain (user's virtual drive)
///
/// File/directory metadata is managed off-chain by the provider node (fs_index).
/// Only drive lifecycle (create/delete) and storage parameters live on-chain.
#[derive(Clone, Encode, Decode, Eq, PartialEq, Debug, TypeInfo, MaxEncodedLen)]
#[scale_info(skip_type_params(MaxNameLength, Balance))]
#[codec(mel_bound())]
pub struct DriveInfo<
    AccountId: Encode + Decode + MaxEncodedLen,
    BlockNumber: Encode + Decode + MaxEncodedLen,
    MaxNameLength: Get<u32>,
> {
    /// Owner of the drive
    pub owner: AccountId,
    /// Layer 0 bucket ID this drive uses
    pub bucket_id: u64,
    /// Block number when drive was created
    pub created_at: BlockNumber,
    /// Optional human-readable name (bounded)
    pub name: Option<BoundedVec<u8, MaxNameLength>>,
    /// Maximum storage capacity in bytes
    pub max_capacity: u64,
    /// Storage period in blocks
    pub storage_period: BlockNumber,
    /// Expiry block number (created_at + storage_period)
    pub expires_at: BlockNumber,
}

// ============================================================================
// Utility functions
// ============================================================================

/// CID of a blob: its Layer 0 `data_root`.
///
/// Splits `data` into 256 KiB chunks (empty data is one empty chunk), hashes
/// each chunk with blake2-256, and returns the root of
/// `storage_primitives::padded_merkle_tree` over the chunk hashes. For a blob
/// of at most 256 KiB this is the blake2-256 hash of the blob.
pub fn compute_cid(data: &[u8]) -> Cid {
    storage_primitives::padded_merkle_tree(&chunk_hashes(data)).0
}

/// blake2-256 hashes of the 256 KiB chunks of `data`, in order. Empty data is
/// one empty chunk.
pub fn chunk_hashes(data: &[u8]) -> Vec<Cid> {
    if data.is_empty() {
        return alloc::vec![storage_primitives::blake2_256(&[])];
    }
    data.chunks(CHUNK_SIZE)
        .map(storage_primitives::blake2_256)
        .collect()
}

/// Check an entry name: 1..=256 bytes of UTF-8, no `/`, not `.` or `..`.
pub fn validate_entry_name(name: &[u8]) -> Result<(), FileSystemError> {
    let valid = !name.is_empty()
        && name.len() <= MaxEntryNameLength::get() as usize
        && core::str::from_utf8(name).is_ok()
        && !name.contains(&b'/')
        && name != b"."
        && name != b"..";
    if valid {
        Ok(())
    } else {
        Err(FileSystemError::InvalidEntryName)
    }
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

    fn h(data: &[u8]) -> H256 {
        storage_primitives::blake2_256(data)
    }

    fn entry(name: &str, entry_type: EntryType, size: u64) -> DirectoryEntry {
        DirectoryEntry::try_new(name, entry_type, h(name.as_bytes()), size, 1).unwrap()
    }

    /// The TypeScript client reads directory and manifest blobs up to these
    /// sizes (`DIRECTORY_NODE_MAX_SIZE`, `FILE_MANIFEST_MAX_SIZE` in
    /// `packages/core/src/file-system.ts`) and asserts the same values.
    #[test]
    fn max_encoded_len_matches_typescript() {
        use codec::MaxEncodedLen;
        assert_eq!(DirectoryNode::max_encoded_len(), 335_116);
        assert_eq!(FileManifest::max_encoded_len(), 2_380_698);
    }

    /// Shared test vector: the TypeScript implementation asserts the same
    /// encoding and CID.
    #[test]
    fn directory_node_test_vector() {
        let mut dir = DirectoryNode::new_empty(7);
        dir.upsert_child(
            DirectoryEntry::try_new("docs", EntryType::Directory, h(b"d"), 0, 1_700_000_001)
                .unwrap(),
        )
        .unwrap();
        dir.upsert_child(
            DirectoryEntry::try_new("a.txt", EntryType::File, h(b"a"), 1, 1_700_000_000).unwrap(),
        )
        .unwrap();
        assert_eq!(dir.children[0].name_str(), "a.txt");
        assert_eq!(
            hex::encode(dir.to_scale_bytes()),
            DIRECTORY_NODE_HEX,
            "encoding"
        );
        assert_eq!(cid_to_string(&dir.compute_cid()), DIRECTORY_NODE_CID, "cid");
    }

    /// Shared test vector: the TypeScript implementation asserts the same
    /// encoding and CID.
    #[test]
    fn file_manifest_test_vector() {
        let manifest = FileManifest::for_content(
            7,
            b"a",
            "text/plain",
            vec![MetadataEntry::try_new(b"origin", b"test").unwrap()],
        )
        .unwrap();
        assert_eq!(manifest.chunks[0].cid, h(b"a"));
        assert_eq!(manifest.content_root(), h(b"a"));
        assert_eq!(
            hex::encode(manifest.to_scale_bytes()),
            FILE_MANIFEST_HEX,
            "encoding"
        );
        assert_eq!(
            cid_to_string(&manifest.compute_cid()),
            FILE_MANIFEST_CID,
            "cid"
        );
    }

    const DIRECTORY_NODE_HEX: &str = "07000000000000000814612e747874008928aae63c84d87ea098564d1e03ad813f107add474e56aedd286349c0c03ea4010000000000000000f153650000000010646f63730100d116515f37a4c0ac872096c8b7412c80693cc5cee2e99e83a7e760dc1ece91000000000000000001f153650000000000";
    const DIRECTORY_NODE_CID: &str =
        "0x2a05cb11a13c3962071e9843a1840b8f2a7948c2b404481f01e252d4f3ce6af9";
    const FILE_MANIFEST_HEX: &str = "070000000000000028746578742f706c61696e0100000000000000048928aae63c84d87ea098564d1e03ad813f107add474e56aedd286349c0c03ea4000000000004186f726967696e1074657374";
    const FILE_MANIFEST_CID: &str =
        "0x4060dec5812cb094e14f3566f175faa0aeda15346f731a971a5c75ad6a345a02";

    #[test]
    fn compute_cid_is_the_padded_data_root() {
        assert_eq!(compute_cid(b""), h(b""));
        assert_eq!(compute_cid(b"hello"), h(b"hello"));
        let exact = vec![1u8; CHUNK_SIZE];
        assert_eq!(compute_cid(&exact), h(&exact));

        let data: Vec<u8> = (0..CHUNK_SIZE * 2 + 5).map(|i| i as u8).collect();
        let leaves = vec![
            h(&data[..CHUNK_SIZE]),
            h(&data[CHUNK_SIZE..CHUNK_SIZE * 2]),
            h(&data[CHUNK_SIZE * 2..]),
        ];
        assert_eq!(chunk_hashes(&data), leaves);
        let expected = storage_primitives::hash_children(
            storage_primitives::hash_children(leaves[0], leaves[1]),
            storage_primitives::hash_children(leaves[2], H256::zero()),
        );
        assert_eq!(compute_cid(&data), expected);
        assert_ne!(compute_cid(&data), h(&data));
    }

    #[test]
    fn manifest_of_empty_content_has_one_empty_chunk() {
        let manifest = FileManifest::for_content(1, b"", DEFAULT_MIME_TYPE, vec![]).unwrap();
        assert_eq!(manifest.total_size, 0);
        assert_eq!(manifest.chunks.len(), 1);
        assert_eq!(manifest.chunks[0].cid, h(b""));
        assert_eq!(manifest.content_root(), compute_cid(b""));
    }

    #[test]
    fn manifest_content_root_matches_content_cid() {
        let data: Vec<u8> = (0..CHUNK_SIZE * 3 + 1).map(|i| (i % 7) as u8).collect();
        let manifest = FileManifest::for_content(1, &data, DEFAULT_MIME_TYPE, vec![]).unwrap();
        assert_eq!(manifest.chunks.len(), 4);
        assert_eq!(manifest.chunks[3].sequence, 3);
        assert_eq!(manifest.content_root(), compute_cid(&data));
    }

    #[test]
    fn empty_mime_type_is_the_default() {
        let manifest = FileManifest::for_content(1, b"x", "", vec![]).unwrap();
        assert_eq!(manifest.mime_type.to_vec(), DEFAULT_MIME_TYPE.as_bytes());

        let mut stored = manifest.clone();
        stored.mime_type = BoundedVec::default();
        assert_eq!(stored.mime_type_str(), DEFAULT_MIME_TYPE);
    }

    #[test]
    fn manifest_validate_checks_chunks_against_size() {
        for len in [0, 1, CHUNK_SIZE, CHUNK_SIZE + 1] {
            let data = vec![1u8; len];
            let manifest = FileManifest::for_content(1, &data, DEFAULT_MIME_TYPE, vec![]).unwrap();
            assert_eq!(manifest.validate(), Ok(()));
        }

        let manifest =
            FileManifest::for_content(1, &[1u8; CHUNK_SIZE + 1], DEFAULT_MIME_TYPE, vec![])
                .unwrap();

        let mut wrong_size = manifest.clone();
        wrong_size.total_size = CHUNK_SIZE as u64;
        assert_eq!(wrong_size.validate(), Err(FileSystemError::InvalidManifest));
        wrong_size.total_size = 2 * CHUNK_SIZE as u64 + 1;
        assert_eq!(wrong_size.validate(), Err(FileSystemError::InvalidManifest));
        wrong_size.total_size = u64::MAX;
        assert_eq!(wrong_size.validate(), Err(FileSystemError::InvalidManifest));

        let mut no_chunks = manifest.clone();
        no_chunks.total_size = 0;
        no_chunks.chunks = BoundedVec::default();
        assert_eq!(no_chunks.validate(), Err(FileSystemError::InvalidManifest));

        let mut wrong_sequence = manifest;
        wrong_sequence.chunks[1].sequence = 0;
        assert_eq!(
            wrong_sequence.validate(),
            Err(FileSystemError::InvalidManifest)
        );
    }

    #[test]
    fn manifest_rejects_values_over_bounds() {
        let long_mime = "a".repeat(129);
        assert_eq!(
            FileManifest::for_content(1, b"x", &long_mime, vec![]),
            Err(FileSystemError::BoundsExceeded)
        );
        let too_many = (0..65)
            .map(|i| MetadataEntry::try_new(format!("k{i}").as_bytes(), b"v").unwrap())
            .collect();
        assert_eq!(
            FileManifest::for_content(1, b"x", DEFAULT_MIME_TYPE, too_many),
            Err(FileSystemError::BoundsExceeded)
        );
        assert!(MetadataEntry::try_new(&[b'k'; 65], b"v").is_err());
        assert!(MetadataEntry::try_new(b"k", &[b'v'; 257]).is_err());
    }

    #[test]
    fn scale_round_trip() {
        let mut dir = DirectoryNode::new_empty(123);
        dir.upsert_child(entry("file1.txt", EntryType::File, 1024))
            .unwrap();
        let decoded = DirectoryNode::from_scale_bytes(&dir.to_scale_bytes()).unwrap();
        assert_eq!(decoded, dir);

        let manifest = FileManifest::for_content(
            123,
            b"content",
            "text/plain",
            vec![MetadataEntry::try_new(b"k", b"v").unwrap()],
        )
        .unwrap();
        let decoded = FileManifest::from_scale_bytes(&manifest.to_scale_bytes()).unwrap();
        assert_eq!(decoded, manifest);
    }

    #[test]
    fn decode_enforces_bounds() {
        // A name of 257 bytes: compact length 257 = 0x0405, then the bytes.
        let mut bytes = Encode::encode(&7u64);
        bytes.extend(Encode::encode(&codec::Compact(1u32)));
        bytes.extend(Encode::encode(&codec::Compact(257u32)));
        bytes.extend([b'a'; 257]);
        bytes.push(0);
        bytes.extend([0u8; 32 + 8 + 8]);
        bytes.extend(Encode::encode(&codec::Compact(0u32)));
        assert!(DirectoryNode::from_scale_bytes(&bytes).is_err());
    }

    #[test]
    fn entry_name_rules() {
        assert!(validate_entry_name(b"a").is_ok());
        assert!(validate_entry_name(&[b'a'; 256]).is_ok());
        assert!(validate_entry_name("ä.txt".as_bytes()).is_ok());
        for bad in [
            &b""[..],
            &[b'a'; 257][..],
            b"a/b",
            b".",
            b"..",
            &[0xff, 0xfe][..],
        ] {
            assert_eq!(
                validate_entry_name(bad),
                Err(FileSystemError::InvalidEntryName)
            );
        }
        assert!(DirectoryEntry::try_new("a/b", EntryType::File, h(b"x"), 0, 0).is_err());
    }

    #[test]
    fn upsert_keeps_children_sorted_and_unique() {
        let mut dir = DirectoryNode::new_empty(1);
        for name in ["b", "a", "c", "B", "aa"] {
            dir.upsert_child(entry(name, EntryType::File, 1)).unwrap();
        }
        let names: Vec<String> = dir.children.iter().map(|e| e.name_str()).collect();
        assert_eq!(names, ["B", "a", "aa", "b", "c"]);

        let mut replacement = entry("aa", EntryType::File, 99);
        replacement.cid = h(b"new");
        dir.upsert_child(replacement).unwrap();
        assert_eq!(dir.children.len(), 5);
        assert_eq!(dir.find_child("aa").unwrap().size, 99);
        assert_eq!(dir.find_child("aa").unwrap().cid, h(b"new"));

        assert!(dir.remove_child("a").is_some());
        assert!(dir.remove_child("a").is_none());
        assert!(dir.find_child("a").is_none());
        dir.validate().unwrap();
    }

    #[test]
    fn upsert_rejects_more_than_max_children() {
        let mut dir = DirectoryNode::new_empty(1);
        for i in 0..MaxDirectoryChildren::get() {
            dir.upsert_child(entry(&format!("{i:05}"), EntryType::File, 0))
                .unwrap();
        }
        assert_eq!(
            dir.upsert_child(entry("overflow", EntryType::File, 0)),
            Err(FileSystemError::BoundsExceeded)
        );
    }

    #[test]
    fn validate_rejects_bad_directories() {
        let mut unsorted = DirectoryNode::new_empty(1);
        unsorted.children = BoundedVec::try_from(vec![
            entry("b", EntryType::File, 0),
            entry("a", EntryType::File, 0),
        ])
        .unwrap();
        assert_eq!(unsorted.validate(), Err(FileSystemError::InvalidDirectory));

        let mut duplicate = DirectoryNode::new_empty(1);
        duplicate.children = BoundedVec::try_from(vec![
            entry("a", EntryType::File, 0),
            entry("a", EntryType::Directory, 0),
        ])
        .unwrap();
        assert_eq!(duplicate.validate(), Err(FileSystemError::InvalidDirectory));

        let mut sized_dir = DirectoryNode::new_empty(1);
        sized_dir.children =
            BoundedVec::try_from(vec![entry("d", EntryType::Directory, 5)]).unwrap();
        assert_eq!(sized_dir.validate(), Err(FileSystemError::InvalidDirectory));

        let mut bad_name = DirectoryNode::new_empty(1);
        let mut e = entry("a", EntryType::File, 0);
        e.name = BoundedVec::try_from(b"..".to_vec()).unwrap();
        bad_name.children = BoundedVec::try_from(vec![e]).unwrap();
        assert_eq!(bad_name.validate(), Err(FileSystemError::InvalidEntryName));

        let mut with_metadata = DirectoryNode::new_empty(1);
        with_metadata.metadata =
            BoundedVec::try_from(vec![MetadataEntry::try_new(b"k", b"v").unwrap()]).unwrap();
        assert_eq!(
            with_metadata.validate(),
            Err(FileSystemError::InvalidDirectory)
        );
    }

    #[test]
    fn test_cid_string_conversion() {
        let cid = compute_cid(b"test");
        let s = cid_to_string(&cid);
        let decoded = string_to_cid(&s).unwrap();
        assert_eq!(cid, decoded);
    }

    #[cfg(feature = "std")]
    #[test]
    fn proto_round_trip() {
        let mut dir = DirectoryNode::new_empty(456);
        dir.upsert_child(entry("test.txt", EntryType::File, 512))
            .unwrap();
        let decoded = DirectoryNode::from_proto_bytes(&dir.to_proto_bytes().unwrap()).unwrap();
        assert_eq!(decoded, dir);

        let manifest = FileManifest::for_content(
            456,
            b"content",
            "text/plain",
            vec![
                MetadataEntry::try_new(b"z", b"1").unwrap(),
                MetadataEntry::try_new(b"a", b"2").unwrap(),
            ],
        )
        .unwrap();
        let decoded = FileManifest::from_proto_bytes(&manifest.to_proto_bytes().unwrap()).unwrap();
        assert_eq!(decoded, manifest);
    }
}
