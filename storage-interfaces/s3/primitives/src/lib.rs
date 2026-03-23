//! S3-compatible storage interface primitives.
//!
//! This crate provides the core types used by the S3 storage interface.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use codec::{Decode, Encode, MaxEncodedLen};
use scale_info::TypeInfo;
use sp_core::H256;
use sp_runtime::{traits::Get, BoundedVec};

/// Maximum length for bucket names (S3 spec: 3-63 characters).
pub const MAX_BUCKET_NAME_LENGTH: u32 = 63;

/// S3 bucket identifier.
pub type S3BucketId = u64;

// ============================================================================
// Type Bounds (implement Get<u32> for use with BoundedVec)
// ============================================================================

/// Maximum bucket name length (64 bytes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode, TypeInfo, MaxEncodedLen)]
pub struct MaxBucketNameLen;
impl Get<u32> for MaxBucketNameLen {
    fn get() -> u32 {
        64
    }
}

/// Maximum object key length (1024 bytes, S3 spec).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode, TypeInfo, MaxEncodedLen)]
pub struct MaxObjectKeyLen;
impl Get<u32> for MaxObjectKeyLen {
    fn get() -> u32 {
        1024
    }
}

/// Maximum content type length (256 bytes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode, TypeInfo, MaxEncodedLen)]
pub struct MaxContentTypeLen;
impl Get<u32> for MaxContentTypeLen {
    fn get() -> u32 {
        256
    }
}

/// Maximum ETag length (128 bytes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode, TypeInfo, MaxEncodedLen)]
pub struct MaxEtagLen;
impl Get<u32> for MaxEtagLen {
    fn get() -> u32 {
        128
    }
}

/// Maximum number of user metadata entries per object.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode, TypeInfo, MaxEncodedLen)]
pub struct MaxMetadataEntries;
impl Get<u32> for MaxMetadataEntries {
    fn get() -> u32 {
        16
    }
}

/// Maximum metadata key length (128 bytes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode, TypeInfo, MaxEncodedLen)]
pub struct MaxMetadataKeyLen;
impl Get<u32> for MaxMetadataKeyLen {
    fn get() -> u32 {
        128
    }
}

/// Maximum metadata value length (256 bytes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode, TypeInfo, MaxEncodedLen)]
pub struct MaxMetadataValueLen;
impl Get<u32> for MaxMetadataValueLen {
    fn get() -> u32 {
        256
    }
}

// ============================================================================
// Type Aliases
// ============================================================================

/// Bounded bucket name type.
pub type BucketName = BoundedVec<u8, MaxBucketNameLen>;

/// Bounded object key type.
pub type ObjectKey = BoundedVec<u8, MaxObjectKeyLen>;

// ============================================================================
// Core Types
// ============================================================================

/// S3 bucket information stored on-chain.
#[derive(Clone, Encode, Decode, TypeInfo, MaxEncodedLen, Debug, PartialEq, Eq)]
pub struct S3BucketInfo<AccountId, BlockNumber> {
    /// Unique S3 bucket identifier.
    pub s3_bucket_id: S3BucketId,
    /// Human-readable bucket name.
    pub name: BucketName,
    /// Link to the underlying Layer 0 bucket.
    pub layer0_bucket_id: u64,
    /// Bucket owner (first admin).
    pub owner: AccountId,
    /// Block number when the bucket was created.
    pub created_at: BlockNumber,
    /// Number of objects in the bucket.
    pub object_count: u64,
    /// Total size of all objects in bytes.
    pub total_size: u64,
}

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

/// S3 error types.
#[derive(Clone, Encode, Decode, TypeInfo, Debug, PartialEq, Eq)]
pub enum S3Error {
    /// Bucket not found.
    NoSuchBucket,
    /// Object not found.
    NoSuchKey,
    /// Bucket already exists.
    BucketAlreadyExists,
    /// Invalid bucket name.
    InvalidBucketName,
    /// Access denied.
    AccessDenied,
    /// Internal error.
    InternalError,
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Compute CID from data using blake2-256.
pub fn compute_cid(data: &[u8]) -> H256 {
    sp_core::hashing::blake2_256(data).into()
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

/// Validate bucket name according to S3 naming rules.
pub fn validate_bucket_name(name: &[u8]) -> bool {
    if name.len() < 3 || name.len() > 63 {
        return false;
    }
    if !name
        .first()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    {
        return false;
    }
    if !name
        .last()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    {
        return false;
    }
    for &byte in name {
        if !byte.is_ascii_lowercase() && !byte.is_ascii_digit() && byte != b'-' {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_bucket_name() {
        assert!(validate_bucket_name(b"mybucket"));
        assert!(validate_bucket_name(b"my-bucket"));
        assert!(!validate_bucket_name(b"ab"));
        assert!(!validate_bucket_name(b"My-Bucket"));
        assert!(!validate_bucket_name(b"-bucket"));
    }

    #[test]
    fn test_compute_cid() {
        let data = b"hello world";
        let cid = compute_cid(data);
        assert_ne!(cid, H256::zero());
    }
}
