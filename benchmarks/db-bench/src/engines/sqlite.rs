//! SQLite backend (B-tree + WAL), via bundled libsqlite3. A single table
//! `kv(k BLOB PRIMARY KEY, v BLOB)` models the keyspace.
//!
//! **Durability mapping.** `synchronous = NORMAL` in WAL mode is the relaxed
//! setting the content store runs at: commits do not fsync, and durability
//! arrives at the next checkpoint. A `sync` batch instead commits at
//! `synchronous = FULL`, which fsyncs the WAL — exactly what the commitment
//! store does in production per `05-per-bucket-store-design.md`. `flush()`
//! remains a `wal_checkpoint(TRUNCATE)`, i.e. the flush barrier.
//!
//! Earlier passes mapped a `sync` batch to a *checkpoint* rather than an fsync.
//! That is strictly more work than production does — it fsyncs and then folds
//! the whole WAL back into the main file on every batch — so those passes
//! understated SQLite's durable-write throughput. The mode is switched only when
//! it actually changes, so a scenario with a constant `sync` flag pays for one
//! PRAGMA, not one per batch.
//!
//! **Page size is a measured variable.** At the 4 KiB default a 256 KiB chunk
//! lands in a ~64-page overflow chain that is walked page by page on read, which
//! is the whole of SQLite's poor content-store read latency. Raising it shortens
//! the chain proportionally, but a larger page is also the *floor* for an empty
//! bucket and the granularity for 48-byte commitment rows — so it trades chunk
//! reads against per-bucket overhead, in both directions. [`set_page_size`] makes
//! that trade measurable instead of assumed.

use super::KvStore;
use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// `0` = leave SQLite's compiled-in default (4096) untouched.
static PAGE_SIZE: AtomicUsize = AtomicUsize::new(0);
/// `0` = leave SQLite's default page cache (2 MiB).
static CACHE_SIZE_KIB: AtomicUsize = AtomicUsize::new(0);
/// `0` = leave mmap disabled, SQLite's default.
static MMAP_SIZE_BYTES: AtomicUsize = AtomicUsize::new(0);
/// When set, keys and payloads live in separate B-trees. See [`set_split_index`].
static SPLIT_INDEX: AtomicBool = AtomicBool::new(false);

/// Set the `page_size` applied to subsequently created stores. `0` restores the
/// engine default. Must be a power of two between 512 and 65536.
pub fn set_page_size(bytes: usize) {
    PAGE_SIZE.store(bytes, Ordering::Relaxed);
}

/// The `page_size` currently applied to newly created stores (`0` = default).
pub fn page_size() -> usize {
    PAGE_SIZE.load(Ordering::Relaxed)
}

/// Set the page cache in KiB (`0` = SQLite's 2 MiB default).
///
/// Decisive for absent-key lookups on a large store: a random B-tree descent
/// touches interior pages that fall out of a small cache once the database
/// outgrows it, turning each miss into real I/O.
pub fn set_cache_size_kib(kib: usize) {
    CACHE_SIZE_KIB.store(kib, Ordering::Relaxed);
}

pub fn cache_size_kib() -> usize {
    CACHE_SIZE_KIB.load(Ordering::Relaxed)
}

/// Set `mmap_size` in bytes (`0` = disabled, SQLite's default).
///
/// With mmap enabled SQLite reads pages straight from the OS page cache instead
/// of `pread`-ing them into its own — the same mechanism that makes LMDB's
/// lookups cheap. The per-instance address-space cost is the trade.
pub fn set_mmap_size(bytes: usize) {
    MMAP_SIZE_BYTES.store(bytes, Ordering::Relaxed);
}

pub fn mmap_size() -> usize {
    MMAP_SIZE_BYTES.load(Ordering::Relaxed)
}

/// Store keys and payloads in separate B-trees.
///
/// The default schema — `kv(k BLOB PRIMARY KEY, v BLOB) WITHOUT ROWID` — puts the
/// row *in* the index, so a 256 KiB payload sits on the same pages a key search
/// descends. Measured, that makes an absent-key lookup ~76× more expensive than
/// the identical key set with 48-byte values, while 50× more keys costs almost
/// nothing: the cost follows bytes, not cardinality.
///
/// The split schema keeps the payload in a rowid table and gives the hash its own
/// index, containing only `(hash, rowid)` pairs. A `check_exists` then descends a
/// B-tree holding no payload at all.
pub fn set_split_index(split: bool) {
    SPLIT_INDEX.store(split, Ordering::Relaxed);
}

pub fn split_index() -> bool {
    SPLIT_INDEX.load(Ordering::Relaxed)
}

/// Apply the cache and mmap settings to a freshly opened connection.
fn apply_cache_settings(connection: &Connection) {
    let cache_kib = cache_size_kib();
    if cache_kib > 0 {
        // Negative values are KiB rather than pages.
        connection
            .pragma_update(None, "cache_size", -(cache_kib as i64))
            .expect("set cache_size");
    }
    let mmap = mmap_size();
    if mmap > 0 {
        connection
            .pragma_update(None, "mmap_size", mmap as i64)
            .expect("set mmap_size");
    }
}

/// Apply the configured page size to a freshly opened connection.
///
/// Must run *before* `journal_mode = WAL`: SQLite refuses to change the page
/// size of a database already in WAL mode without a `VACUUM`, so the order here
/// is load-bearing rather than stylistic.
pub(super) fn apply_page_size_for_shared(connection: &Connection) {
    apply_page_size(connection);
}

fn apply_page_size(connection: &Connection) {
    let configured = page_size();
    if configured == 0 {
        return;
    }
    connection
        .pragma_update(None, "page_size", configured as i64)
        .expect("set page_size");
}

pub struct SqliteStore {
    connection: Connection,
    /// Mirrors the connection's current `synchronous` setting so the PRAGMA is
    /// only issued when the requested durability actually changes.
    full_sync: bool,
}

impl SqliteStore {
    pub fn open(path: &Path) -> Self {
        // One file per store; the bucket model maps a bucket to this file.
        let connection = Connection::open(path.join("db.sqlite")).expect("open sqlite");
        apply_page_size(&connection);
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .expect("set WAL");
        connection
            .pragma_update(None, "synchronous", "NORMAL")
            .expect("set synchronous");
        apply_cache_settings(&connection);
        if split_index() {
            connection
                .execute_batch(
                    "CREATE TABLE IF NOT EXISTS kv (id INTEGER PRIMARY KEY, k BLOB NOT NULL, v BLOB NOT NULL);\n\
                     CREATE UNIQUE INDEX IF NOT EXISTS kv_k ON kv (k);",
                )
                .expect("create split tables");
        } else {
            connection
                .execute(
                    "CREATE TABLE IF NOT EXISTS kv (k BLOB PRIMARY KEY, v BLOB NOT NULL) WITHOUT ROWID",
                    [],
                )
                .expect("create table");
        }
        Self {
            connection,
            full_sync: false,
        }
    }
}

/// Switch a connection's `synchronous` mode, returning the new state. Split out
/// so the single-threaded store and the shared concurrent writer cannot drift.
pub(super) fn set_full_sync(connection: &Connection, full_sync: bool) {
    connection
        .pragma_update(
            None,
            "synchronous",
            if full_sync { "FULL" } else { "NORMAL" },
        )
        .expect("set synchronous");
}

impl KvStore for SqliteStore {
    fn commit_batch(&mut self, batch: &[(Vec<u8>, Vec<u8>)], sync: bool) {
        if sync != self.full_sync {
            set_full_sync(&self.connection, sync);
            self.full_sync = sync;
        }
        let transaction = self.connection.transaction().expect("sqlite transaction");
        {
            let mut statement = transaction
                .prepare_cached(if split_index() {
                    "INSERT INTO kv (k, v) VALUES (?1, ?2)
                     ON CONFLICT(k) DO UPDATE SET v = excluded.v"
                } else {
                    "INSERT OR REPLACE INTO kv (k, v) VALUES (?1, ?2)"
                })
                .expect("prepare");
            for (key, value) in batch {
                statement
                    .execute(params![key, value])
                    .expect("sqlite batch insert");
            }
        }
        // Committing at `synchronous = FULL` has already fsynced the WAL; the
        // checkpoint belongs to `flush`, not to every durable batch.
        transaction.commit().expect("sqlite commit");
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

    /// `VACUUM` rewrites the database into a fresh file with no free pages,
    /// returning the slack to the filesystem.
    fn compact(&mut self) -> bool {
        self.flush();
        self.connection
            .execute_batch("VACUUM")
            .expect("sqlite vacuum");
        true
    }
}
