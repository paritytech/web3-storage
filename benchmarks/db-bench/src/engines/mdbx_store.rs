//! libmdbx backend (LMDB descendant, memory-mapped copy-on-write B+tree).
//!
//! Included to test whether mdbx's improvements over LMDB survive the thing this
//! suite actually cares about — being instantiated once per bucket. On paper it
//! removes both LMDB liabilities that bear on a database-per-bucket design:
//!
//! 1. **No `map_size` ceiling to pick.** mdbx's geometry is dynamic: it grows on
//!    demand and, given a `shrink_threshold`, returns space to the filesystem.
//!    LMDB forces a per-environment ceiling chosen up front, which for buckets
//!    holding chunked media is a guess that either wastes reserved address space
//!    or fails with `MDB_MAP_FULL`.
//! 2. **`MDBX_NOTLS` is unconditional** in this wrapper (see
//!    `Database::make_flags` in libmdbx 0.6), so the thread-local-storage
//!    exhaustion that caps how many LMDB environments one process can hold open
//!    does not apply.
//!
//! **Measured, both advantages are outweighed by a third property mdbx does not
//! share with LMDB: it spawns one background transaction-manager thread per
//! environment.** At 1000 open environments the harness records 1000 extra
//! threads and ~12.9 GiB of reserved address space *each* — mdbx picks its own
//! generous geometry when `max_size` is `None`, so "no ceiling to choose" turns
//! into "a large ceiling chosen for you". That makes it a poor fit for an LRU
//! pool over thousands of buckets, which is the opposite of the reason it was
//! added. Keep it in the matrix as the measured counter-example.
//!
//! Auto-shrink is still worth having measured, since it speaks to the finding
//! that runs through every report here: no other engine reclaims space on a bare
//! delete. mdbx is the one candidate that claims to.
//!
//! Durability mapping: opened `SyncMode::SafeNoSync` so a plain `commit()` is a
//! genuine non-durable commit, with `sync(true)` forcing the fsync when the
//! caller asks. This mirrors the `heed`/SQLite/redb split and keeps the `sync`
//! flag meaningful. mdbx's default `SyncMode::Durable` would fsync every commit
//! and make the two indistinguishable.

use super::KvStore;
use libmdbx::{
    Database, DatabaseOptions, Mode, NoWriteMap, ReadWriteOptions, SyncMode, WriteFlags,
};
use std::path::Path;

/// Return space to the filesystem once this many bytes are unused.
const SHRINK_THRESHOLD: isize = 16 * 1024 * 1024;
/// Grow the map in steps of this size rather than doubling from tiny.
const GROWTH_STEP: isize = 16 * 1024 * 1024;

pub struct MdbxStore {
    db: Database<NoWriteMap>,
}

impl MdbxStore {
    pub fn open(path: &Path) -> Self {
        Self { db: open_db(path) }
    }
}

/// Open (creating if needed) an mdbx database with dynamic, self-shrinking
/// geometry and no fixed upper bound.
pub fn open_db(path: &Path) -> Database<NoWriteMap> {
    std::fs::create_dir_all(path).expect("mdbx directory");
    let options = DatabaseOptions {
        max_tables: Some(1),
        mode: Mode::ReadWrite(ReadWriteOptions {
            // Durability is forced explicitly in `flush` so an unsynced batch is
            // genuinely unsynced (see the module docs).
            sync_mode: SyncMode::SafeNoSync,
            // `None` leaves mdbx to choose: no ceiling, no preallocation.
            min_size: None,
            max_size: None,
            growth_step: Some(GROWTH_STEP),
            shrink_threshold: Some(SHRINK_THRESHOLD),
        }),
        ..Default::default()
    };
    Database::open_with_options(path, options).expect("open mdbx database")
}

/// Write `batch` in one transaction, fsyncing afterwards when `sync` is set.
pub fn commit_batch(db: &Database<NoWriteMap>, batch: &[(Vec<u8>, Vec<u8>)], sync: bool) {
    let txn = db.begin_rw_txn().expect("mdbx write transaction");
    {
        let table = txn
            .create_table(None, Default::default())
            .expect("mdbx table");
        for (key, value) in batch {
            txn.put(&table, key, value, WriteFlags::UPSERT)
                .expect("mdbx put");
        }
    }
    txn.commit().expect("mdbx commit");
    if sync {
        db.sync(true).expect("mdbx sync");
    }
}

impl KvStore for MdbxStore {
    fn commit_batch(&mut self, batch: &[(Vec<u8>, Vec<u8>)], sync: bool) {
        commit_batch(&self.db, batch, sync);
    }

    fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        let txn = self.db.begin_ro_txn().expect("mdbx read transaction");
        let table = txn.open_table(None).expect("mdbx open table");
        txn.get::<Vec<u8>>(&table, key).expect("mdbx get")
    }

    fn delete(&mut self, key: &[u8]) {
        let txn = self.db.begin_rw_txn().expect("mdbx delete transaction");
        {
            let table = txn.open_table(None).expect("mdbx open table");
            txn.del(&table, key, None).expect("mdbx delete");
        }
        txn.commit().expect("mdbx delete commit");
    }

    fn flush(&mut self) {
        self.db.sync(true).expect("mdbx sync");
    }

    /// mdbx reclaims through its own garbage collector and the `shrink_threshold`
    /// set at open — both driven by commits, not by an explicit call. By this
    /// trait's definition that is *not* a compaction API, so this reports
    /// `false`: the disk-size numbers must not be read as post-compaction. A
    /// durable sync is still issued, since it is the closest thing to "settle
    /// now", which is what lets the disk figures show whether the automatic
    /// shrink returned bytes on its own.
    fn compact(&mut self) -> bool {
        self.flush();
        false
    }
}
