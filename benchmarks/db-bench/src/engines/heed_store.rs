//! LMDB backend via `heed` (memory-mapped copy-on-write B+tree).
//!
//! LMDB is the reference design for the question this suite is really asking:
//! *how cheap is it to have one database per bucket?* Opening an environment is
//! an `mmap` plus a read of one meta page, and LMDB spawns **no background
//! threads at all** — the two properties that make an engine cheap to multiply.
//!
//! Two LMDB-specific costs the other engines do not have, both of which this
//! backend is set up to expose rather than hide:
//!
//! 1. **`map_size` is a ceiling chosen up front.** LMDB maps the whole thing at
//!    open and returns `MDB_MAP_FULL` when the data outgrows it; raising it means
//!    reopening. With one environment per bucket, `map_size × open_instances` is
//!    reserved virtual address space, so the pool cap and the per-bucket ceiling
//!    are coupled. [`set_map_size`] makes it a measured variable.
//! 2. **Read transactions use thread-local storage by default**, and TLS keys are
//!    a fixed per-process resource — LMDB returns `MDB_TLS_FULL` once too many
//!    environments are open. We open with [`EnvOpenOptions::read_txn_without_tls`]
//!    (`MDB_NOTLS`), without which the sharded model would hit a wall well before
//!    the FD or memory limits. See `mdbx_store` for the engine that does this
//!    unconditionally.
//!
//! Durability mapping: LMDB fsyncs on every commit by default, which would make
//! its unsynced batches indistinguishable from its synced ones. To match the
//! `sync` flag the other engines honour, the environment is opened `NO_SYNC |
//! NO_META_SYNC` and durability is forced explicitly via `force_sync()` — so a
//! `sync` batch is a real fsync and an unsynced batch is a real write-back defer,
//! the same split SQLite (`synchronous = NORMAL`) and redb (`Durability::None`)
//! get.

use super::KvStore;
use heed::types::Bytes;
use heed::{Database, Env, EnvFlags, EnvOpenOptions, WithoutTls};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Default per-environment `map_size`: 1 GiB.
///
/// Deliberately modest so the standard scenarios stay runnable at 1000
/// instances; the point of LMDB here is the *coupling* between this number and
/// the pool cap, which the dedicated probe measures rather than assumes.
const DEFAULT_MAP_SIZE: usize = 1024 * 1024 * 1024;

static MAP_SIZE: AtomicUsize = AtomicUsize::new(DEFAULT_MAP_SIZE);

/// Override the per-environment `map_size` for subsequently opened stores.
pub fn set_map_size(bytes: usize) {
    MAP_SIZE.store(bytes, Ordering::Relaxed);
}

/// The `map_size` currently applied to newly opened environments.
pub fn map_size() -> usize {
    MAP_SIZE.load(Ordering::Relaxed)
}

pub struct HeedStore {
    env: Env<WithoutTls>,
    db: Database<Bytes, Bytes>,
}

impl HeedStore {
    pub fn open(path: &Path) -> Self {
        let (env, db) = open_env(path);
        Self { env, db }
    }
}

/// Open (creating if needed) the environment and its single unnamed database.
///
/// Returns both because `heed` ties the `Database` handle's lifetime to the
/// environment, so callers must keep them together.
pub fn open_env(path: &Path) -> (Env<WithoutTls>, Database<Bytes, Bytes>) {
    std::fs::create_dir_all(path).expect("lmdb directory");
    let env = unsafe {
        EnvOpenOptions::new()
            // MDB_NOTLS: read transactions must not consume a thread-local
            // storage key, or many open environments exhaust the process's
            // supply and LMDB starts returning MDB_TLS_FULL.
            .read_txn_without_tls()
            .map_size(map_size())
            .max_dbs(1)
            .max_readers(128)
            // Durability is forced explicitly in `flush`, so that an unsynced
            // batch is genuinely unsynced (see the module docs).
            .flags(EnvFlags::NO_SYNC | EnvFlags::NO_META_SYNC)
            .open(path)
    }
    .expect("open lmdb environment");

    let mut write_txn = env.write_txn().expect("lmdb create-db transaction");
    let db = env
        .create_database(&mut write_txn, None)
        .expect("lmdb create database");
    write_txn.commit().expect("lmdb create-db commit");

    (env, db)
}

/// Write `batch` in one transaction, fsyncing afterwards when `sync` is set.
pub fn commit_batch(
    env: &Env<WithoutTls>,
    db: &Database<Bytes, Bytes>,
    batch: &[(Vec<u8>, Vec<u8>)],
    sync: bool,
) {
    let mut write_txn = env.write_txn().expect("lmdb write transaction");
    for (key, value) in batch {
        db.put(&mut write_txn, key.as_slice(), value.as_slice())
            .expect("lmdb put");
    }
    write_txn.commit().expect("lmdb commit");
    if sync {
        env.force_sync().expect("lmdb force sync");
    }
}

impl KvStore for HeedStore {
    fn commit_batch(&mut self, batch: &[(Vec<u8>, Vec<u8>)], sync: bool) {
        commit_batch(&self.env, &self.db, batch, sync);
    }

    fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        let read_txn = self.env.read_txn().expect("lmdb read transaction");
        self.db
            .get(&read_txn, key)
            .expect("lmdb get")
            .map(<[u8]>::to_vec)
    }

    fn delete(&mut self, key: &[u8]) {
        let mut write_txn = self.env.write_txn().expect("lmdb delete transaction");
        self.db.delete(&mut write_txn, key).expect("lmdb delete");
        write_txn.commit().expect("lmdb delete commit");
    }

    fn flush(&mut self) {
        self.env.force_sync().expect("lmdb force sync");
    }

    /// LMDB reuses freed pages within the map but never returns them to the
    /// filesystem — there is no online compaction call (`mdb_copy` with
    /// compaction is an offline, whole-file rewrite). Reported as "no API" so
    /// the disk-size scenarios are not read as "compacted and still this big".
    fn compact(&mut self) -> bool {
        self.flush();
        false
    }
}
