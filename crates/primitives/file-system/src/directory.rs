// SPDX-License-Identifier: Apache-2.0

//! Directory structures: entries, metadata, and the directory node itself
//! (SCALE-encoded, no_std compatible).

use alloc::{string::String, vec::Vec};
use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use scale_info::TypeInfo;
use sp_runtime::BoundedVec;

use crate::{
    compute_cid, Cid, DriveId, MaxDirectoryChildren, MaxEntryNameLength, MaxMetadataEntries,
    MaxMetadataKeyLength, MaxMetadataValueLength,
};

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

    /// Compute the CID (blake2-256 hash) of this directory node
    pub fn compute_cid(&self) -> Cid {
        compute_cid(&self.to_scale_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_directory_encoding() {
        let dir = DirectoryNode::new_empty(2);
        let bytes = dir.to_scale_bytes();
        println!("Empty DirectoryNode for drive_id=2:");
        println!("  Length: {}", bytes.len());
        println!("  Hex: {}", hex::encode(&bytes));
        let cid = compute_cid(&bytes);
        println!("  CID: 0x{}", hex::encode(cid.as_bytes()));

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
