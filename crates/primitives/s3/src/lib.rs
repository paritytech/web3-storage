// SPDX-License-Identifier: Apache-2.0

//! S3-compatible storage interface primitives.
//!
//! This crate provides the core types used by the S3 storage interface.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod bounds;
pub mod bucket;
pub mod error;
pub mod object;

pub use bounds::*;
pub use bucket::*;
pub use error::*;
pub use object::*;

use sp_core::H256;

/// Maximum length for bucket names (S3 spec: 3-63 characters).
pub const MAX_BUCKET_NAME_LENGTH: u32 = 63;

/// S3 bucket identifier.
pub type S3BucketId = u64;

/// Compute CID from data using blake2-256.
pub fn compute_cid(data: &[u8]) -> H256 {
    sp_core::hashing::blake2_256(data).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_cid() {
        let data = b"hello world";
        let cid = compute_cid(data);
        assert_ne!(cid, H256::zero());
    }
}
