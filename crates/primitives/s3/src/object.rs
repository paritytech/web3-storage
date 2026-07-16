// SPDX-License-Identifier: Apache-2.0

//! On-chain object metadata, object key validation, and listing types.

use codec::{Decode, Encode, MaxEncodedLen};
use scale_info::TypeInfo;
use sp_core::H256;
use sp_runtime::BoundedVec;

use crate::{
    MaxContentTypeLen, MaxEtagLen, MaxMetadataEntries, MaxMetadataKeyLen, MaxMetadataValueLen,
    MaxObjectKeyLen,
};

/// Bounded object key type.
pub type ObjectKey = BoundedVec<u8, MaxObjectKeyLen>;

/// A single user metadata key-value pair stored on-chain.
#[derive(Clone, Encode, Decode, TypeInfo, MaxEncodedLen, Debug, PartialEq, Eq)]
pub struct MetadataEntry {
    /// Metadata key.
    pub key: BoundedVec<u8, MaxMetadataKeyLen>,
    /// Metadata value.
    pub value: BoundedVec<u8, MaxMetadataValueLen>,
}

/// Object metadata stored on-chain.
#[derive(Clone, Encode, Decode, TypeInfo, MaxEncodedLen, Debug, PartialEq, Eq)]
pub struct ObjectMetadata {
    /// Content identifier (blake2-256 hash of data).
    pub cid: H256,
    /// Size of the object in bytes.
    pub size: u64,
    /// Last modified timestamp (block number as u64).
    pub last_modified: u64,
    /// MIME content type.
    pub content_type: BoundedVec<u8, MaxContentTypeLen>,
    /// ETag (hex-encoded CID).
    pub etag: BoundedVec<u8, MaxEtagLen>,
    /// User-defined metadata entries.
    pub user_metadata: BoundedVec<MetadataEntry, MaxMetadataEntries>,
}

/// Parameters for listing objects (S3 ListObjectsV2 style).
#[cfg(feature = "std")]
#[derive(Clone, Debug, Default)]
pub struct ListObjectsParams {
    /// Filter objects by key prefix.
    pub prefix: Option<alloc::string::String>,
    /// Delimiter for grouping keys into common prefixes.
    pub delimiter: Option<alloc::string::String>,
    /// Maximum number of keys to return.
    pub max_keys: Option<u32>,
    /// Continuation token for pagination.
    pub continuation_token: Option<alloc::string::String>,
}

/// Response from listing objects.
#[cfg(feature = "std")]
#[derive(Clone, Debug)]
pub struct ListObjectsResponse {
    /// Bucket name.
    pub name: alloc::vec::Vec<u8>,
    /// Prefix filter used.
    pub prefix: Option<alloc::string::String>,
    /// Delimiter used.
    pub delimiter: Option<alloc::string::String>,
    /// Maximum number of keys requested.
    pub max_keys: u32,
    /// Whether the result is truncated.
    pub is_truncated: bool,
    /// Token for fetching next page.
    pub next_continuation_token: Option<alloc::string::String>,
    /// Matching object entries.
    pub contents: alloc::vec::Vec<ListObjectEntry>,
    /// Common prefixes (when delimiter is used).
    pub common_prefixes: alloc::vec::Vec<alloc::string::String>,
    /// Number of keys returned.
    pub key_count: u32,
}

/// A single object entry in a list response.
#[cfg(feature = "std")]
#[derive(Clone, Debug)]
pub struct ListObjectEntry {
    /// Object key.
    pub key: alloc::string::String,
    /// Last modified timestamp.
    pub last_modified: u64,
    /// ETag.
    pub etag: alloc::string::String,
    /// Size in bytes.
    pub size: u64,
}

/// Compute ETag from CID (hex string without 0x prefix).
#[cfg(feature = "std")]
pub fn compute_etag(cid: &H256) -> alloc::vec::Vec<u8> {
    hex::encode(cid.as_bytes()).into_bytes()
}

/// Validate object key according to S3 naming rules.
///
/// Keys must be 1-1024 bytes, UTF-8, and not start with '/'.
pub fn validate_object_key(key: &[u8]) -> bool {
    if key.is_empty() || key.len() > 1024 {
        return false;
    }
    // Must be valid UTF-8
    if core::str::from_utf8(key).is_err() {
        return false;
    }
    true
}
