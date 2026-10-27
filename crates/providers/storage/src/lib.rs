// SPDX-License-Identifier: Apache-2.0

//! Storage engine for provider nodes: content-addressed blob storage with MMR
//! commitments, plus the drive/S3 metadata indexes served over it. No HTTP or
//! chain dependencies.
//!
//! Main types:
//! * [`StorageBackend`] : the engine interface. The node holds an `Arc<dyn StorageBackend>`.
//! * [`DiskStorage`] : the RocksDB implementation, built from a [`StorageBackendSpec`].
//! * [`FsIndexManager`], [`S3IndexManager`] : the drive and S3 metadata indexes.
//!
//! Inputs: chunk tree nodes and data roots to commit, per bucket, from the HTTP handlers.
//!
//! Outputs: nodes, chunk data, MMR commitments ([`CommitOutcome`]) and proofs. The challenge
//! responder reads its proofs through the same backend.
pub mod backend;
pub mod error;
pub mod index;

pub use backend::{
    build_merkle_tree, BucketInfo, BucketState, ChunkTreeNode, CommitOutcome, DiskStorage,
    StorageBackend, StorageBackendSpec,
};
pub use error::Error;
pub use index::{
    FsEntryMeta, FsIndexManager, FsListEntry, ListResult, ObjectEntry, ObjectMeta, S3IndexManager,
};

/// Names the scratch directories [`temp_rocksdb`] creates.
#[cfg(any(test, feature = "test-helpers"))]
pub const TEMP_DIR_PREFIX: &str = "w3s-provider-storage-";

/// What [`temp_rocksdb`] returns.
#[cfg(any(test, feature = "test-helpers"))]
pub type TempBackend = (std::sync::Arc<dyn StorageBackend>, tempfile::TempDir);

/// A RocksDB backend on a scratch directory. Keep the guard for as long as the
/// backend is in use — dropping it takes the database with it.
///
/// The directory is prefixed, so anything a test leaks stays greppable.
#[cfg(any(test, feature = "test-helpers"))]
pub fn temp_rocksdb() -> TempBackend {
    let dir = tempfile::Builder::new()
        .prefix(TEMP_DIR_PREFIX)
        .tempdir()
        .expect("temp dir");
    let storage = StorageBackendSpec::RocksDb {
        path: dir.path().to_path_buf(),
    }
    .build()
    .expect("RocksDB opens");
    (storage, dir)
}
