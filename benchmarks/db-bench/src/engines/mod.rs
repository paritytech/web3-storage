//! A minimal key/value abstraction over the four candidate engines so the
//! workloads can be written once and run against each.
//!
//! Fairness note (also stated in the reports): achieving *identical* durability
//! semantics across four engines with different commit pipelines is not
//! possible. We pick a documented, comparable baseline per engine and surface
//! the `sync` flag on batch commits to the strongest equivalent each one
//! offers. The exact per-engine configuration lives in each submodule and is
//! reproduced in `04-configuration-guide.md`.

mod concurrent;
mod paritydb;
mod rocks;
mod sled_store;
mod sqlite;

pub use concurrent::{open_shared_concurrent, Writer};

use clap::ValueEnum;
use std::path::Path;

/// The candidate engines under evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Engine {
    Rocksdb,
    Sled,
    Sqlite,
    Paritydb,
}

impl Engine {
    pub fn name(self) -> &'static str {
        match self {
            Engine::Rocksdb => "rocksdb",
            Engine::Sled => "sled",
            Engine::Sqlite => "sqlite",
            Engine::Paritydb => "paritydb",
        }
    }

    /// Open (creating if needed) a single-keyspace store at `path`.
    pub fn open(self, path: &Path) -> Box<dyn KvStore> {
        match self {
            Engine::Rocksdb => Box::new(rocks::RocksStore::open(path)),
            Engine::Sled => Box::new(sled_store::SledStore::open(path)),
            Engine::Sqlite => Box::new(sqlite::SqliteStore::open(path)),
            Engine::Paritydb => Box::new(paritydb::ParityStore::open(path)),
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
}
