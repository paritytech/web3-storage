//! SQLite backend (B-tree + WAL), via bundled libsqlite3. A single table
//! `kv(k BLOB PRIMARY KEY, v BLOB)` models the keyspace. WAL mode with
//! `synchronous = NORMAL` is the standard durable-but-fast configuration; a
//! `sync` batch additionally checkpoints the WAL.

use super::KvStore;
use rusqlite::{params, Connection};
use std::path::Path;

pub struct SqliteStore {
    connection: Connection,
}

impl SqliteStore {
    pub fn open(path: &Path) -> Self {
        // One file per store; the bucket model maps a bucket to this file.
        let connection = Connection::open(path.join("db.sqlite")).expect("open sqlite");
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .expect("set WAL");
        connection
            .pragma_update(None, "synchronous", "NORMAL")
            .expect("set synchronous");
        connection
            .execute(
                "CREATE TABLE IF NOT EXISTS kv (k BLOB PRIMARY KEY, v BLOB NOT NULL) WITHOUT ROWID",
                [],
            )
            .expect("create table");
        Self { connection }
    }
}

impl KvStore for SqliteStore {
    fn commit_batch(&mut self, batch: &[(Vec<u8>, Vec<u8>)], sync: bool) {
        let transaction = self.connection.transaction().expect("sqlite transaction");
        {
            let mut statement = transaction
                .prepare_cached("INSERT OR REPLACE INTO kv (k, v) VALUES (?1, ?2)")
                .expect("prepare");
            for (key, value) in batch {
                statement
                    .execute(params![key, value])
                    .expect("sqlite batch insert");
            }
        }
        transaction.commit().expect("sqlite commit");
        if sync {
            self.connection
                .pragma_update(None, "wal_checkpoint", "TRUNCATE")
                .expect("wal checkpoint");
        }
    }

    fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        self.connection
            .query_row("SELECT v FROM kv WHERE k = ?1", params![key], |row| {
                row.get::<_, Vec<u8>>(0)
            })
            .ok()
    }

    fn delete(&mut self, key: &[u8]) {
        self.connection
            .execute("DELETE FROM kv WHERE k = ?1", params![key])
            .expect("sqlite delete");
    }

    fn flush(&mut self) {
        self.connection
            .pragma_update(None, "wal_checkpoint", "TRUNCATE")
            .expect("wal checkpoint");
    }
}
