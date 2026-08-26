//! Concurrent-writer abstraction for the **shared-DB** architecture comparison.
//!
//! The single most decisive difference between one DB per bucket or one single
//! shared DB for all buckets is how each engine behaves when many buckets are
//! written concurrently through *one* database:
//!
//! - RocksDB / Sled / ParityDB are internally concurrent — many threads share
//!   one handle and the engine serializes only as much as it must.
//! - SQLite is single-writer: every thread opens its own connection to the same
//!   WAL file, but writes serialize on the database write lock. This models the
//!   real ceiling of a shared-SQLite design.
//!
//! `Shared` is the cross-thread handle (one per database); `new_writer()`
//! produces a per-thread `Writer`. For SQLite each writer is an independent
//! connection; for the others each writer shares the one underlying DB.

use super::Engine;
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A per-thread writer into a shared database.
pub trait Writer: Send {
    fn commit_batch(&mut self, batch: &[(Vec<u8>, Vec<u8>)], sync: bool);
}

/// A cross-thread handle to one shared database.
pub trait Shared: Send + Sync {
    fn new_writer(&self) -> Box<dyn Writer>;
}

/// Open (creating if needed) a single shared database that hands out concurrent
/// writers.
pub fn open_shared_concurrent(engine: Engine, path: &Path) -> Arc<dyn Shared> {
    match engine {
        Engine::Rocksdb => Arc::new(RocksShared::open(path)),
        Engine::Sled => Arc::new(SledShared::open(path)),
        Engine::Sqlite => Arc::new(SqliteShared::open(path)),
        Engine::Paritydb => Arc::new(ParityShared::open(path)),
        Engine::Redb => Arc::new(RedbShared::open(path)),
        // LMDB, mdbx and jammdb are single-writer mmap'd B+trees evaluated for
        // the per-bucket-file model only; they are absent from the shared-DB
        // candidate list, so this handle is never requested for them.
        Engine::Lmdb | Engine::Mdbx | Engine::Jammdb => {
            unreachable!("{} is a sharded-only candidate", engine.name())
        }
    }
}

// ── RocksDB: one DB handle shared across threads ──────────────────────────────

struct RocksShared {
    db: Arc<rocksdb::DB>,
}
impl RocksShared {
    fn open(path: &Path) -> Self {
        let mut options = rocksdb::Options::default();
        options.create_if_missing(true);
        options.set_write_buffer_size(8 * 1024 * 1024);
        options.set_max_write_buffer_number(2);
        options.set_max_background_jobs(2);
        options.set_keep_log_file_num(1);
        let db = rocksdb::DB::open(&options, path).expect("open rocksdb shared");
        Self { db: Arc::new(db) }
    }
}
impl Shared for RocksShared {
    fn new_writer(&self) -> Box<dyn Writer> {
        Box::new(RocksWriter {
            db: self.db.clone(),
        })
    }
}
struct RocksWriter {
    db: Arc<rocksdb::DB>,
}
impl Writer for RocksWriter {
    fn commit_batch(&mut self, batch: &[(Vec<u8>, Vec<u8>)], sync: bool) {
        let mut write_batch = rocksdb::WriteBatch::default();
        for (key, value) in batch {
            write_batch.put(key, value);
        }
        let mut write_options = rocksdb::WriteOptions::default();
        write_options.set_sync(sync);
        self.db
            .write_opt(write_batch, &write_options)
            .expect("rocksdb shared write");
    }
}

// ── Sled: cheaply-cloneable Db shared across threads ──────────────────────────

struct SledShared {
    db: sled::Db,
}
impl SledShared {
    fn open(path: &Path) -> Self {
        let db = sled::Config::new()
            .path(path)
            .flush_every_ms(None)
            .open()
            .expect("open sled shared");
        Self { db }
    }
}
impl Shared for SledShared {
    fn new_writer(&self) -> Box<dyn Writer> {
        Box::new(SledWriter {
            db: self.db.clone(),
        })
    }
}
struct SledWriter {
    db: sled::Db,
}
impl Writer for SledWriter {
    fn commit_batch(&mut self, batch: &[(Vec<u8>, Vec<u8>)], sync: bool) {
        let mut sled_batch = sled::Batch::default();
        for (key, value) in batch {
            sled_batch.insert(key.clone(), value.clone());
        }
        self.db.apply_batch(sled_batch).expect("sled shared batch");
        if sync {
            self.db.flush().expect("sled shared flush");
        }
    }
}

// ── ParityDB: one Db shared across threads ────────────────────────────────────

struct ParityShared {
    db: Arc<parity_db::Db>,
}
impl ParityShared {
    fn open(path: &Path) -> Self {
        let options = parity_db::Options::with_columns(path, 1);
        let db = parity_db::Db::open_or_create(&options).expect("open parity-db shared");
        Self { db: Arc::new(db) }
    }
}
impl Shared for ParityShared {
    fn new_writer(&self) -> Box<dyn Writer> {
        Box::new(ParityWriter {
            db: self.db.clone(),
        })
    }
}
struct ParityWriter {
    db: Arc<parity_db::Db>,
}
impl Writer for ParityWriter {
    fn commit_batch(&mut self, batch: &[(Vec<u8>, Vec<u8>)], _sync: bool) {
        let changes = batch
            .iter()
            .map(|(key, value)| (0u8, key.clone(), Some(value.clone())));
        self.db.commit(changes).expect("parity-db shared commit");
    }
}

// ── redb: one Database shared across threads, single writer at a time ─────────
//
// redb enforces its single-writer rule in-process: `begin_write` blocks on a
// condvar until the live write transaction finishes. That is the same
// serialization SQLite gets from its file lock, so the shared-vs-sharded gap
// here is directly comparable to SQLite's.

struct RedbShared {
    db: Arc<redb::Database>,
}
impl RedbShared {
    fn open(path: &Path) -> Self {
        let db = redb::Database::create(path.join("db.redb")).expect("open redb shared");
        super::redb_store::create_table(&db);
        Self { db: Arc::new(db) }
    }
}
impl Shared for RedbShared {
    fn new_writer(&self) -> Box<dyn Writer> {
        Box::new(RedbWriter {
            db: self.db.clone(),
        })
    }
}
struct RedbWriter {
    db: Arc<redb::Database>,
}
impl Writer for RedbWriter {
    fn commit_batch(&mut self, batch: &[(Vec<u8>, Vec<u8>)], sync: bool) {
        super::redb_store::commit_batch(&self.db, batch, sync);
    }
}

// ── SQLite: a connection per thread to the same WAL file ──────────────────────

struct SqliteShared {
    path: PathBuf,
}
impl SqliteShared {
    fn open(path: &Path) -> Self {
        let file = path.join("db.sqlite");
        // Pre-create the file + table once so per-thread writers just attach.
        let connection = Connection::open(&file).expect("open sqlite shared");
        super::sqlite::apply_page_size_for_shared(&connection);
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .unwrap();
        connection
            .pragma_update(None, "synchronous", "NORMAL")
            .unwrap();
        connection
            .execute(
                "CREATE TABLE IF NOT EXISTS kv (k BLOB PRIMARY KEY, v BLOB NOT NULL) WITHOUT ROWID",
                [],
            )
            .unwrap();
        Self { path: file }
    }
}
impl Shared for SqliteShared {
    fn new_writer(&self) -> Box<dyn Writer> {
        let connection = Connection::open(&self.path).expect("open sqlite writer connection");
        connection
            .pragma_update(None, "synchronous", "NORMAL")
            .unwrap();
        // Block (rather than error) when another writer holds the lock — this is
        // exactly the single-writer serialization we want to measure.
        connection
            .busy_timeout(std::time::Duration::from_secs(30))
            .unwrap();
        Box::new(SqliteWriter {
            connection,
            full_sync: false,
        })
    }
}
struct SqliteWriter {
    connection: Connection,
    full_sync: bool,
}
impl Writer for SqliteWriter {
    fn commit_batch(&mut self, batch: &[(Vec<u8>, Vec<u8>)], sync: bool) {
        // Must honour `sync` identically to the sharded store: when this writer
        // ignored the flag it made the shared architecture look far faster than
        // sharded purely because it was skipping the durability the other side
        // was paying for.
        if sync != self.full_sync {
            super::sqlite::set_full_sync(&self.connection, sync);
            self.full_sync = sync;
        }
        let transaction = self
            .connection
            .transaction()
            .expect("sqlite writer transaction");
        {
            let mut statement = transaction
                .prepare_cached("INSERT OR REPLACE INTO kv (k, v) VALUES (?1, ?2)")
                .expect("prepare");
            for (key, value) in batch {
                statement
                    .execute(rusqlite::params![key, value])
                    .expect("sqlite writer insert");
            }
        }
        transaction.commit().expect("sqlite writer commit");
    }
}
