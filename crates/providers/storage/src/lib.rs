// SPDX-License-Identifier: Apache-2.0

//! Storage engine for provider nodes: content-addressed blob storage with MMR
//! commitments, plus the drive/S3 metadata indexes served over it. No HTTP or
//! chain dependencies.
//!
pub mod backend;
pub mod error;
pub mod index;
pub mod merkle;
pub mod mmr;
pub mod nonce;

#[cfg(any(test, feature = "test-helpers"))]
pub use backend::Storage;
pub use backend::{
    build_padded_merkle_tree, BucketInfo, BucketStats, BucketSummary, DiskNonceStore, DiskStorage,
    OpenedBackend, StorageBackend, StorageBackendSpec, StoredNode,
};
pub use error::Error;
pub use index::{
    FsEntryMeta, FsIndexManager, FsListEntry, ListResult, ObjectEntry, ObjectMeta, S3IndexManager,
};
pub use merkle::build_merkle_proof;
pub use nonce::{NonceStore, NullNonceStore};
