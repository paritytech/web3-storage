//! Sled backend (lock-free B-tree, pure Rust). We disable sled's periodic
//! background flush so durability is controlled explicitly via `flush()` /
//! `sync` batches, matching how the other engines are measured.

use super::KvStore;
use sled::{Batch, Db};
use std::path::Path;

pub struct SledStore {
    db: Db,
}

impl SledStore {
    pub fn open(path: &Path) -> Self {
        // Sled holds an advisory file lock that is not always released the
        // instant a prior `Db` is dropped, so reopening the same path back to
        // back can transiently fail with `WouldBlock`. Retry briefly. (This
        // reopen friction is itself noted as a finding in the report.)
        let mut last_error = None;
        for attempt in 0..40 {
            match sled::Config::new()
                .path(path)
                .flush_every_ms(None) // explicit flushes only
                .open()
            {
                Ok(db) => return Self { db },
                Err(error) => {
                    last_error = Some(error);
                    std::thread::sleep(std::time::Duration::from_millis(25.min(2 + attempt)));
                }
            }
        }
        panic!("open sled: {:?}", last_error.unwrap());
    }
}

impl KvStore for SledStore {
    fn commit_batch(&mut self, batch: &[(Vec<u8>, Vec<u8>)], sync: bool) {
        let mut sled_batch = Batch::default();
        for (key, value) in batch {
            sled_batch.insert(key.clone(), value.clone());
        }
        self.db.apply_batch(sled_batch).expect("sled apply_batch");
        if sync {
            self.db.flush().expect("sled flush");
        }
    }

    fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        self.db
            .get(key)
            .expect("sled get")
            .map(|value| value.to_vec())
    }

    fn delete(&mut self, key: &[u8]) {
        self.db.remove(key).expect("sled remove");
    }

    fn flush(&mut self) {
        self.db.flush().expect("sled flush");
    }

    /// Sled exposes no manual compaction; its GC runs on its own schedule.
    /// Reported as "no API" so the disk numbers are not read as "compacted and
    /// still this large".
    fn compact(&mut self) -> bool {
        self.flush();
        false
    }
}
