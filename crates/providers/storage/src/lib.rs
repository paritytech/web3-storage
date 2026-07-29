// SPDX-License-Identifier: Apache-2.0

//! Storage engine for provider nodes: content-addressed blob storage with MMR
//! commitments, plus the drive/S3 metadata indexes served over it. No HTTP or
//! chain dependencies.
//!
//! Layout:
//! - [`backend`]: the [`StorageBackend`] trait and its two implementations -
//!   [`Storage`] (in-memory, for development and tests) and [`DiskStorage`]
//!   (persistent, RocksDB) - plus [`build_padded_merkle_tree`], which writes
//!   through the trait.
//! - [`index`]: metadata layered over the blobs - file-system drives
//!   ([`index::fs`]) and S3-style object listings ([`index::s3`]).
//! - [`mmr`], [`merkle`]: pure commitment/proof math used by the backends.
//! - [`error`]: the crate's [`Error`] type.
//! - [`nonce`]: persistence for the negotiation nonce high-water mark.

pub mod backend;
pub mod error;
pub mod index;
pub mod merkle;
pub mod mmr;
pub mod nonce;

pub use backend::{
    build_padded_merkle_tree, BucketInfo, BucketStats, BucketSummary, DiskNonceStore, DiskStorage,
    Storage, StorageBackend, StoredNode,
};
pub use error::Error;
pub use index::{
    FsEntryMeta, FsIndexManager, FsListEntry, ListResult, ObjectEntry, ObjectMeta, S3IndexManager,
};
pub use merkle::build_merkle_proof;
pub use nonce::{NonceStore, NullNonceStore};
