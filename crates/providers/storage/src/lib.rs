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

pub use backend::{
    build_padded_merkle_tree, BucketInfo, BucketStats, BucketSummary, DiskNonceStore, DiskStorage,
    OpenedBackend, StorageBackend, StorageBackendSpec, StoredNode,
};
pub use error::Error;
pub use index::{
    FsEntryMeta, FsIndexManager, FsListEntry, ListResult, ObjectEntry, ObjectMeta, S3IndexManager,
};
pub use merkle::build_merkle_proof;
pub use nonce::NonceStore;

/// What [`temp_rocksdb`] returns.
#[cfg(any(test, feature = "test-helpers"))]
pub type TempBackend = (
    std::sync::Arc<dyn StorageBackend>,
    std::sync::Arc<dyn NonceStore>,
    tempfile::TempDir,
);

/// A RocksDB backend on a scratch directory. Keep the guard for as long as the
/// backend is in use — dropping it takes the database with it.
#[cfg(any(test, feature = "test-helpers"))]
pub fn temp_rocksdb() -> TempBackend {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let (storage, nonce_store) = StorageBackendSpec::RocksDb {
        path: dir.path().to_path_buf(),
    }
    .build()
    .expect("RocksDB opens");
    (storage, nonce_store, dir)
}
