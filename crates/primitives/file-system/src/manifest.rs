// SPDX-License-Identifier: Apache-2.0

//! File manifests: chunk references and reassembly metadata
//! (SCALE-encoded, no_std compatible).

use alloc::{string::String, vec::Vec};
use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use scale_info::TypeInfo;
use sp_runtime::BoundedVec;

use crate::{
    compute_cid, Cid, DriveId, MaxEncryptionParamsLength, MaxFileChunks, MaxMimeTypeLength,
};

/// A single chunk reference in a file (SCALE-encoded, no_std compatible)
#[derive(
    Clone, Encode, Decode, DecodeWithMemTracking, Eq, PartialEq, Debug, TypeInfo, MaxEncodedLen,
)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub struct FileChunk {
    /// Chunk CID (blake2-256 hash)
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
