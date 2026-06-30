//! RocksDB backend (LSM-tree). Configured close to a lightweight per-instance
//! profile: small write buffers and block cache, since the per-bucket model
//! opens many instances. The state-trie workload uses the same options.

use super::KvStore;
use rocksdb::{Options, WriteBatch, WriteOptions, DB};
use std::path::Path;

pub struct RocksStore {
    db: DB,
}

impl RocksStore {
    pub fn open(path: &Path) -> Self {
        let mut options = Options::default();
        options.create_if_missing(true);
        // Lightweight per-instance footprint: a small write buffer and a couple
        // of background jobs. Larger production tuning lives in the config guide.
        options.set_write_buffer_size(8 * 1024 * 1024); // 8 MiB memtable
        options.set_max_write_buffer_number(2);
        options.set_max_background_jobs(2);
        options.set_keep_log_file_num(1);
        let db = DB::open(&options, path).expect("open rocksdb");
        Self { db }
    }
}

impl KvStore for RocksStore {
    fn commit_batch(&mut self, batch: &[(Vec<u8>, Vec<u8>)], sync: bool) {
        let mut write_batch = WriteBatch::default();
        for (key, value) in batch {
            write_batch.put(key, value);
        }
        let mut write_options = WriteOptions::default();
        write_options.set_sync(sync);
        self.db
            .write_opt(write_batch, &write_options)
            .expect("rocksdb batch write");
    }

    fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        self.db.get(key).expect("rocksdb get")
    }

    fn delete(&mut self, key: &[u8]) {
        self.db.delete(key).expect("rocksdb delete");
    }

    fn flush(&mut self) {
        self.db.flush().expect("rocksdb flush");
    }
}
