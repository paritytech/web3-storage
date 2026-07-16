// SPDX-License-Identifier: Apache-2.0

//! On-chain S3 bucket types and bucket name validation.

use codec::{Decode, Encode, MaxEncodedLen};
use scale_info::TypeInfo;
use sp_runtime::BoundedVec;

use crate::{MaxBucketNameLen, S3BucketId};

/// Bounded bucket name type.
pub type BucketName = BoundedVec<u8, MaxBucketNameLen>;

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
}
