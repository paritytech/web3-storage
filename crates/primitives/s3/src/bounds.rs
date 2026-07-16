// SPDX-License-Identifier: Apache-2.0

//! Bound marker types (implement `Get<u32>` for use with `BoundedVec`).

use codec::{Decode, Encode, MaxEncodedLen};
use scale_info::TypeInfo;
use sp_runtime::traits::Get;

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
