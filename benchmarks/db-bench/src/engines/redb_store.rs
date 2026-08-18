//! redb backend (copy-on-write B-tree, pure Rust). The same index family as
//! SQLite and Sled, but reached through a typed KV API instead of SQL — which
//! makes it the direct test of "how much is the SQL layer costing us?".
//!
//! Durability mapping: redb's `Durability::Immediate` fsyncs before `commit()`
//! returns, so it is the honest equivalent of RocksDB's `WriteOptions::sync`
//! and SQLite's WAL checkpoint. A non-`sync` batch uses `Durability::None`,
//! which defers persistence until the next immediate commit — hence `flush()`
//! issues an empty immediate commit to force it.
//!
//! Like SQLite, redb allows one writer at a time; unlike SQLite it enforces
//! that in-process on a condvar rather than through a file lock, and readers
//! never block (MVCC).

use super::KvStore;
use redb::{Database, Durability, ReadableDatabase, TableDefinition};
use std::path::Path;

/// Single keyspace, mirroring the `kv(k, v)` table the SQLite backend uses.
const TABLE: TableDefinition<&[u8], &[u8]> = TableDefinition::new("kv");

pub struct RedbStore {
    db: Database,
}

impl RedbStore {
    pub fn open(path: &Path) -> Self {
        let db = Database::create(path.join("db.redb")).expect("open redb");
        create_table(&db);
        Self { db }
    }
}

/// Materialize the table so `get` on a freshly-opened store sees an empty table
/// rather than `TableDoesNotExist`.
pub fn create_table(db: &Database) {
    let transaction = db.begin_write().expect("redb create-table transaction");
    transaction.open_table(TABLE).expect("redb open table");
    transaction.commit().expect("redb create-table commit");
}

/// Write `batch` in one transaction at the durability level `sync` selects.
/// Shared by the single-threaded store and the concurrent-writer handle.
pub fn commit_batch(db: &Database, batch: &[(Vec<u8>, Vec<u8>)], sync: bool) {
    let mut transaction = db.begin_write().expect("redb write transaction");
    transaction
        .set_durability(if sync {
            Durability::Immediate
        } else {
            Durability::None
        })
        .expect("redb set durability");
    {
        let mut table = transaction.open_table(TABLE).expect("redb open table");
        for (key, value) in batch {
            table
                .insert(key.as_slice(), value.as_slice())
                .expect("redb insert");
        }
    }
    transaction.commit().expect("redb commit");
}

impl KvStore for RedbStore {
    fn commit_batch(&mut self, batch: &[(Vec<u8>, Vec<u8>)], sync: bool) {
        commit_batch(&self.db, batch, sync);
    }

    fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        let transaction = self.db.begin_read().expect("redb read transaction");
        let table = transaction.open_table(TABLE).expect("redb open table");
        table
            .get(key)
            .expect("redb get")
            .map(|guard| guard.value().to_vec())
    }

    fn delete(&mut self, key: &[u8]) {
        let transaction = self.db.begin_write().expect("redb delete transaction");
        {
            let mut table = transaction.open_table(TABLE).expect("redb open table");
            table.remove(key).expect("redb remove");
        }
        transaction.commit().expect("redb delete commit");
    }

    fn flush(&mut self) {
        // An immediate-durability commit persists everything written by any
        // preceding `Durability::None` commit.
        commit_batch(&self.db, &[], true);
    }

    /// redb's copy-on-write B-tree leaves superseded pages on the free list;
    /// `compact` rewrites the file to drop them. It reclaims incrementally and
    /// reports `false` once no further compaction is possible, so drive it to a
    /// fixed point (bounded, so a pathological case cannot spin forever).
    fn compact(&mut self) -> bool {
        self.flush();
        for _ in 0..16 {
            if !self.db.compact().expect("redb compact") {
                break;
            }
        }
        true
    }
}
