//! jammdb backend (pure-Rust BoltDB port: single mmap'd file, B+tree).
//!
//! The pure-Rust member of the LMDB family. Same shape as LMDB — one file,
//! memory-mapped, copy-on-write B+tree, single writer, lock-free readers, **no
//! background threads** — but without the C dependency and without LMDB's
//! `map_size` ceiling, since it grows the file as needed.
//!
//! It is here to answer a narrow question: does the cheap-to-open profile come
//! from the mmap'd single-file B+tree design itself, or from LMDB's specific
//! two-decade-old C implementation? jammdb is the same design at a fraction of
//! the maturity, so a large gap in reopen latency or disk amplification points at
//! the implementation rather than the architecture.
//!
//! Durability mapping: jammdb exposes no per-commit durability knob — `commit()`
//! writes and fsyncs. So the `sync` flag cannot be honoured downward: every
//! batch here is fully durable, which makes its unsynced numbers pessimistic
//! against engines that can defer. Flagged in the reports wherever it matters.

use super::KvStore;
use jammdb::DB;
use std::path::Path;

/// Single keyspace, mirroring the `kv(k, v)` table the other backends use.
const BUCKET: &str = "kv";

pub struct JammdbStore {
    db: DB,
}

impl JammdbStore {
    pub fn open(path: &Path) -> Self {
        Self { db: open_db(path) }
    }
}

/// Open (creating if needed) the database file and materialize its bucket, so a
/// `get` on a freshly-opened store sees an empty bucket rather than an error.
pub fn open_db(path: &Path) -> DB {
    std::fs::create_dir_all(path).expect("jammdb directory");
    let db = DB::open(path.join("db.jammdb")).expect("open jammdb");
    let tx = db.tx(true).expect("jammdb create-bucket transaction");
    tx.get_or_create_bucket(BUCKET)
        .expect("jammdb create bucket");
    tx.commit().expect("jammdb create-bucket commit");
    db
}

/// Write `batch` in one transaction. `sync` is ignored: see the module docs.
pub fn commit_batch(db: &DB, batch: &[(Vec<u8>, Vec<u8>)], _sync: bool) {
    let tx = db.tx(true).expect("jammdb write transaction");
    {
        let bucket = tx.get_or_create_bucket(BUCKET).expect("jammdb bucket");
        for (key, value) in batch {
            bucket
                .put(key.as_slice(), value.as_slice())
                .expect("jammdb put");
        }
    }
    tx.commit().expect("jammdb commit");
}

impl KvStore for JammdbStore {
    fn commit_batch(&mut self, batch: &[(Vec<u8>, Vec<u8>)], sync: bool) {
        commit_batch(&self.db, batch, sync);
    }

    fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        let tx = self.db.tx(false).expect("jammdb read transaction");
        let bucket = tx.get_bucket(BUCKET).expect("jammdb bucket");
        bucket.get(key).map(|data| data.kv().value().to_vec())
    }

    fn delete(&mut self, key: &[u8]) {
        let tx = self.db.tx(true).expect("jammdb delete transaction");
        {
            let bucket = tx.get_or_create_bucket(BUCKET).expect("jammdb bucket");
            // Deleting an absent key is not an error for this harness.
            let _ = bucket.delete(key);
        }
        tx.commit().expect("jammdb delete commit");
    }

    fn flush(&mut self) {
        // Every commit is already durable; there is nothing buffered to force.
    }
}
