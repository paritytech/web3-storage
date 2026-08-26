//! A minimal key/value abstraction over the candidate engines so the
//! workloads can be written once and run against each.
//!
//! Fairness note (also stated in the reports): achieving *identical* durability
//! semantics across engines with different commit pipelines is not
//! possible. We pick a documented, comparable baseline per engine and surface
//! the `sync` flag on batch commits to the strongest equivalent each one
//! offers. The exact per-engine configuration lives in each submodule and is
//! reproduced in `03-configuration-guide.md`.

mod concurrent;
mod heed_store;
mod jammdb_store;
mod mdbx_store;
mod paritydb;
mod redb_store;
mod rocks;
mod sled_store;
mod sqlite;

pub use concurrent::{open_shared_concurrent, Writer};
pub use heed_store::{map_size as lmdb_map_size, set_map_size as set_lmdb_map_size};
pub use sqlite::{
    cache_size_kib as sqlite_cache_size_kib, mmap_size as sqlite_mmap_size,
    page_size as sqlite_page_size, set_cache_size_kib as set_sqlite_cache_size_kib,
    set_mmap_size as set_sqlite_mmap_size, set_page_size as set_sqlite_page_size,
    set_split_index as set_sqlite_split_index, split_index as sqlite_split_index,
};

use clap::ValueEnum;
use std::path::Path;

/// The candidate engines under evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Engine {
    Rocksdb,
    Sled,
    Sqlite,
    Paritydb,
    /// Pure-Rust copy-on-write B-tree — SQLite's index family without SQL.
    Redb,
    /// LMDB via `heed` — mmap'd B+tree, zero background threads. Sharded only.
    Lmdb,
    /// libmdbx — LMDB descendant with dynamic, self-shrinking geometry. Sharded only.
    Mdbx,
    /// jammdb — pure-Rust BoltDB port, the same design as LMDB. Sharded only.
    Jammdb,
}

impl Engine {
    pub fn name(self) -> &'static str {
        match self {
            Engine::Rocksdb => "rocksdb",
            Engine::Sled => "sled",
            Engine::Sqlite => "sqlite",
            Engine::Paritydb => "paritydb",
            Engine::Redb => "redb",
            Engine::Lmdb => "lmdb",
            Engine::Mdbx => "mdbx",
            Engine::Jammdb => "jammdb",
        }
    }

    /// Open (creating if needed) a single-keyspace store at `path`.
    pub fn open(self, path: &Path) -> Box<dyn KvStore> {
        match self {
            Engine::Rocksdb => Box::new(rocks::RocksStore::open(path)),
            Engine::Sled => Box::new(sled_store::SledStore::open(path)),
            Engine::Sqlite => Box::new(sqlite::SqliteStore::open(path)),
            Engine::Paritydb => Box::new(paritydb::ParityStore::open(path)),
            Engine::Redb => Box::new(redb_store::RedbStore::open(path)),
            Engine::Lmdb => Box::new(heed_store::HeedStore::open(path)),
            Engine::Mdbx => Box::new(mdbx_store::MdbxStore::open(path)),
            Engine::Jammdb => Box::new(jammdb_store::JammdbStore::open(path)),
        }
    }
}

/// A single-keyspace, byte-oriented key/value store.
///
/// Methods take `&mut self` so engines that need unique access for batched
/// transactions (SQLite) fit the same interface; the harness is single-threaded
/// per store instance.
pub trait KvStore {
    /// Atomic batch write. When `sync` is set, the engine makes the strongest
    /// durability guarantee it cheaply can (fsync / WAL checkpoint).
    fn commit_batch(&mut self, batch: &[(Vec<u8>, Vec<u8>)], sync: bool);

    fn get(&self, key: &[u8]) -> Option<Vec<u8>>;

    /// Delete a single key.
    fn delete(&mut self, key: &[u8]);

    /// Flush all buffered state durably to disk.
    fn flush(&mut self);

    /// Reclaim free space held by superseded pages, tombstones, or free lists.
    ///
    /// Returns whether the engine has an explicit reclamation API at all. This
    /// distinction matters when reading the disk-size scenarios: `false` means
    /// the post-compaction size is simply the pre-compaction size, *not* that
    /// compaction was tried and found nothing. Engines that reclaim only via a
    /// background worker (ParityDB) or offer no API (Sled) return `false`.
    fn compact(&mut self) -> bool {
        false
    }
}
