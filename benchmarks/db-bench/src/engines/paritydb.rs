//! ParityDB backend (hash-indexed value tables, pure Rust). Single column,
//! variable-length values. ParityDB commits through an asynchronous log +
//! background worker, so there is no per-commit fsync flag to honor; the `sync`
//! argument is therefore a no-op here and that asymmetry is called out in the
//! reports. Dropping the `Db` flushes outstanding state, which the cold-read /
//! disk-size workloads rely on (they reopen the store).

use super::KvStore;
use parity_db::{Db, Options};
use std::path::Path;

const COLUMN: u8 = 0;

pub struct ParityStore {
    db: Db,
}

impl ParityStore {
    pub fn open(path: &Path) -> Self {
        let options = Options::with_columns(path, 1);
        let db = Db::open_or_create(&options).expect("open parity-db");
        Self { db }
    }
}

impl KvStore for ParityStore {
    fn commit_batch(&mut self, batch: &[(Vec<u8>, Vec<u8>)], _sync: bool) {
        let changes = batch
            .iter()
            .map(|(key, value)| (COLUMN, key.clone(), Some(value.clone())));
        self.db.commit(changes).expect("parity-db commit");
    }

    fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        self.db.get(COLUMN, key).expect("parity-db get")
    }

    fn delete(&mut self, key: &[u8]) {
        self.db
            .commit(vec![(COLUMN, key.to_vec(), None)])
            .expect("parity-db delete");
    }

    fn flush(&mut self) {
        // No public flush API; durability is achieved by the background worker
        // and finalized on drop. Workloads that measure on-disk state reopen.
    }
}
