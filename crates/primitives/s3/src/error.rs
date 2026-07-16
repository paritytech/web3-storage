// SPDX-License-Identifier: Apache-2.0

//! S3 error types.

use codec::{Decode, Encode};
use scale_info::TypeInfo;

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
