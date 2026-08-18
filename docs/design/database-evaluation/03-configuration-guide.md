# Configuration Guide

Concrete tuning for the chosen engine — SQLite in WAL mode, one database per
bucket — plus the on-chain key-layout guidance that survives independently of it.

---

## Storage Provider — SQLite (WAL) per bucket

### Per-connection PRAGMAs

Apply on every bucket connection at open (mirrors the harness in
[`benchmarks/db-bench/src/engines/sqlite.rs`](../../../benchmarks/db-bench/src/engines/sqlite.rs)):

```sql
PRAGMA journal_mode = WAL;        -- concurrent readers during a write
PRAGMA synchronous  = NORMAL;     -- durable + fast; full fsync only at checkpoint
PRAGMA busy_timeout = 5000;       -- ride out brief lock contention
PRAGMA cache_size   = -2000;      -- ~2 MiB page cache per connection (negative = KiB)
PRAGMA mmap_size    = 0;          -- avoid per-bucket mmap RAM at thousands of instances
PRAGMA wal_autocheckpoint = 1000; -- checkpoint every ~1000 pages
```

Schema — keep one keyspace, integer-position key, no rowid overhead:

```sql
CREATE TABLE IF NOT EXISTS kv (k BLOB PRIMARY KEY, v BLOB NOT NULL) WITHOUT ROWID;
```

### LRU connection pool

The reason SQLite wins is cheap reopen (37 µs) and low RSS (~32 KiB/instance).
Configure the pool to exploit that:

- **Cap open connections** well below `ulimit -n / 3` (SQLite uses ~3 FDs per
  open DB: main file + `-wal` + `-shm`). At a 65k FD limit, ~10k buckets fit
  comfortably; size the LRU cap to your memory budget first, FDs second.
- **Memory budget:** ≈ `cache_size + ~32 KiB` per live connection. With the 2 MiB
  cache above, 1000 hot buckets ≈ ~2 GiB worst case — lower `cache_size` to
  `-512` (512 KiB) if you expect many simultaneously-hot buckets.
- **Eviction = close.** Closing a connection releases its WAL/shm FDs and cache;
  reopen is 37 µs, so aggressive eviction is cheap.
- **Checkpoint on eviction.** Run `PRAGMA wal_checkpoint(TRUNCATE)` before closing
  a bucket to keep the `-wal` file from growing unbounded across sessions.

### Bucket deletion

Delete the bucket = close the connection and `unlink` the three files
(`<bucket>.sqlite`, `-wal`, `-shm`). Measured ~0.1 ms and reclaims 100% of space
immediately — **do not** issue `DELETE FROM kv` (141 ms and reclaims little; see
[report 01, deletion section](01-storage-provider-benchmark.md#decisive-metric-2--bucket-deletion-favors-sharded-decisively)).

---

## On-chain key-prefix layout

This one is runtime design, not database tuning, and it outlives the engine
choice: it applies to our pallet's storage regardless of which node runs it.

Structure on-chain storage keys so entries sharing a parent (e.g. a Bucket ID)
sort contiguously, enabling bulk range deletion in one pass instead of scattered
tombstones. Concretely, prefer composite keys `(&bucket_id, &item_id)` /
`StorageDoubleMap<BucketId, ItemId, _>` over hashing the pair into one opaque key,
so a bucket's entries form a contiguous range that `clear_prefix` can drop
efficiently. (The shared-DB benchmark uses exactly this `bucket_id || position`
layout.)

> Note: the original plan attributed this to Issue #65, but Issue #65 is the
> "Robust Syncing Protocol for Dynamic Primary and Replica Node Topologies" — a
> different topic. Track key-prefix restructuring under Issue #101 (or a new
> dedicated issue), not under #65.

---

## OS-level checklist

- [ ] `ulimit -n` raised to comfortably exceed `3 × max_open_buckets` (SQLite) on the provider host.
- [ ] LRU pool cap sized against the memory budget first, FDs second.
- [ ] Scheduled compaction/vacuum job — no engine reclaims space on a bare delete.
- [ ] SQLite buckets deleted via file `unlink`, never `DELETE FROM`.
