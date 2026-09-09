// SPDX-License-Identifier: Apache-2.0

//! Metadata indexes layered over the blob storage: per-drive file-system
//! trees ([`fs`]) and S3-style object listings ([`s3`]).

pub mod fs;
pub mod s3;

pub use fs::{FsEntryMeta, FsIndexManager, FsListEntry};
pub use s3::{ListResult, ObjectEntry, ObjectMeta, S3IndexManager};
