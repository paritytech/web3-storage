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

// ============================================================================
// Type Aliases
// ============================================================================

/// Bounded bucket name type.
pub type BucketName = BoundedVec<u8, MaxBucketNameLen>;

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
