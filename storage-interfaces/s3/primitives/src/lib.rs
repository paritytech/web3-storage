//! S3-compatible storage interface primitives.
//!
//! This crate provides the core types used by the S3 storage interface.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::vec::Vec;
use codec::{Decode, Encode, MaxEncodedLen};
use scale_info::TypeInfo;
use sp_core::H256;
use sp_runtime::{traits::Get, BoundedVec};

/// Maximum length for bucket names (S3 spec: 3-63 characters).
pub const MAX_BUCKET_NAME_LENGTH: u32 = 63;

/// Maximum length for object keys (S3 spec: up to 1024 bytes).
pub const MAX_OBJECT_KEY_LENGTH: u32 = 1024;

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

/// Maximum object key length (1024 bytes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode, TypeInfo, MaxEncodedLen)]
pub struct MaxObjectKeyLen;
impl Get<u32> for MaxObjectKeyLen {
	fn get() -> u32 {
		1024
	}
}

/// Maximum content type length (128 bytes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode, TypeInfo, MaxEncodedLen)]
pub struct MaxContentTypeLen;
impl Get<u32> for MaxContentTypeLen {
	fn get() -> u32 {
		128
	}
}

/// Maximum ETag length (64 bytes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode, TypeInfo, MaxEncodedLen)]
pub struct MaxEtagLen;
impl Get<u32> for MaxEtagLen {
	fn get() -> u32 {
		64
	}
}

/// Maximum number of metadata entries per object (16).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode, TypeInfo, MaxEncodedLen)]
pub struct MaxMetadataEntries;
impl Get<u32> for MaxMetadataEntries {
	fn get() -> u32 {
		16
	}
}

/// Maximum metadata key length (64 bytes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode, TypeInfo, MaxEncodedLen)]
pub struct MaxMetadataKeyLen;
impl Get<u32> for MaxMetadataKeyLen {
	fn get() -> u32 {
		64
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

/// Metadata entry (key-value pair).
#[derive(Clone, Encode, Decode, TypeInfo, MaxEncodedLen, Debug, PartialEq, Eq, Default)]
pub struct MetadataEntry {
	/// Metadata key.
	pub key: BoundedVec<u8, MaxMetadataKeyLen>,
	/// Metadata value.
	pub value: BoundedVec<u8, MaxMetadataValueLen>,
}

/// Object metadata stored on-chain.
#[derive(Clone, Encode, Decode, TypeInfo, MaxEncodedLen, Debug, PartialEq, Eq)]
pub struct ObjectMetadata {
	/// Content hash (data root from Layer 0).
	pub cid: H256,
	/// Object size in bytes.
	pub size: u64,
	/// Last modified timestamp (Unix epoch seconds).
	pub last_modified: u64,
	/// Content type (MIME type).
	pub content_type: BoundedVec<u8, MaxContentTypeLen>,
	/// ETag for S3 compatibility (CID hex string).
	pub etag: BoundedVec<u8, MaxEtagLen>,
	/// User-defined metadata.
	pub user_metadata: BoundedVec<MetadataEntry, MaxMetadataEntries>,
}

/// List objects parameters.
#[derive(Clone, Encode, Decode, TypeInfo, Debug, PartialEq, Eq, Default)]
pub struct ListObjectsParams {
	/// Filter by prefix.
	pub prefix: Option<Vec<u8>>,
	/// Delimiter for grouping.
	pub delimiter: Option<u8>,
	/// Continuation token for pagination.
	pub continuation_token: Option<Vec<u8>>,
	/// Maximum keys to return.
	pub max_keys: Option<u32>,
	/// Start after this key.
	pub start_after: Option<Vec<u8>>,
}

/// Object info in list response.
#[derive(Clone, Encode, Decode, TypeInfo, Debug, PartialEq, Eq)]
pub struct ObjectInfo {
	/// Object key.
	pub key: Vec<u8>,
	/// Object size.
	pub size: u64,
	/// Last modified timestamp.
	pub last_modified: u64,
	/// ETag.
	pub etag: Vec<u8>,
}

/// List objects response.
#[derive(Clone, Encode, Decode, TypeInfo, Debug, PartialEq, Eq)]
pub struct ListObjectsResponse {
	/// Bucket name.
	pub name: Vec<u8>,
	/// Prefix used for filtering.
	pub prefix: Option<Vec<u8>>,
	/// Delimiter used.
	pub delimiter: Option<u8>,
	/// Maximum keys requested.
	pub max_keys: u32,
	/// Whether more results are available.
	pub is_truncated: bool,
	/// Token for next page.
	pub next_continuation_token: Option<Vec<u8>>,
	/// Objects matching the criteria.
	pub contents: Vec<ObjectInfo>,
	/// Common prefixes (for delimiter grouping).
	pub common_prefixes: Vec<Vec<u8>>,
	/// Number of keys returned.
	pub key_count: u32,
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
	/// Bucket not empty.
	BucketNotEmpty,
	/// Invalid bucket name.
	InvalidBucketName,
	/// Invalid object key.
	InvalidObjectKey,
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
pub fn compute_etag(cid: &H256) -> Vec<u8> {
	hex::encode(cid.as_bytes()).into_bytes()
}

/// Validate bucket name according to S3 naming rules.
pub fn validate_bucket_name(name: &[u8]) -> bool {
	if name.len() < 3 || name.len() > 63 {
		return false;
	}
	if !name.first().map_or(false, |c| c.is_ascii_lowercase() || c.is_ascii_digit()) {
		return false;
	}
	if !name.last().map_or(false, |c| c.is_ascii_lowercase() || c.is_ascii_digit()) {
		return false;
	}
	for &byte in name {
		if !byte.is_ascii_lowercase() && !byte.is_ascii_digit() && byte != b'-' {
			return false;
		}
	}
	true
}

/// Validate object key.
pub fn validate_object_key(key: &[u8]) -> bool {
	if key.is_empty() || key.len() > 1024 {
		return false;
	}
	!key.contains(&0)
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
	fn test_validate_object_key() {
		assert!(validate_object_key(b"file.txt"));
		assert!(validate_object_key(b"folder/file.txt"));
		assert!(!validate_object_key(b""));
	}

	#[test]
	fn test_compute_cid() {
		let data = b"hello world";
		let cid = compute_cid(data);
		assert_ne!(cid, H256::zero());
	}
}
