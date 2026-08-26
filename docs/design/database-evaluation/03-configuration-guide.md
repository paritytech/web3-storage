# Configuration Guide

Concrete tuning for the chosen engine — SQLite in WAL mode, **two databases per
bucket** per [05-per-bucket-store-design.md](05-per-bucket-store-design.md) — plus
the on-chain key-layout guidance that survives independently of it.

The two stores get **different settings**, and the differences are load-bearing:
the commitment store holds the slashable state and must be fully durable, while
the content store holds hash-verified, re-fetchable chunks and buys throughput
with relaxed per-write durability plus a single barrier.

---

## Storage Provider — SQLite (WAL) per bucket

### Per-connection PRAGMAs

Apply at open, per store (mirrors the harness in
[`benchmarks/db-bench/src/engines/sqlite.rs`](../../../benchmarks/db-bench/src/engines/sqlite.rs)):

```sql
-- Commitment store: MMR leaves + interior nodes + bucket state.
-- 48-byte rows; the slashable half; small and permanently hot.
PRAGMA page_size    = 4096;       -- the default: big pages buy nothing for 48 B rows
PRAGMA journal_mode = WAL;
PRAGMA synchronous  = FULL;       -- fsync the WAL on every commit
PRAGMA busy_timeout = 5000;
PRAGMA cache_size   = -2000;      -- ~2 MiB; the whole store is ~10 MB per 100 k uploads
PRAGMA mmap_size    = 0;
PRAGMA wal_autocheckpoint = 1000;
```

```sql
-- Content store: 256 KiB chunks + chunk-tree interior nodes, hash-keyed.
PRAGMA page_size    = 32768;      -- set FIRST, before WAL; see below
PRAGMA journal_mode = WAL;
PRAGMA synchronous  = NORMAL;     -- durability arrives at the barrier, not per write
PRAGMA busy_timeout = 5000;
PRAGMA cache_size   = -2000;
PRAGMA mmap_size    = 1073741824; -- 1 GiB: worth 4.3x on chunk reads, see below
-- NOTE: SQLite's mmap_size is a *cap*, not a reservation. It maps existing pages
-- up to that limit, so address space grows with the file and 1 GiB here costs
-- nothing on a small bucket. This is unlike LMDB's map_size, which reserves the
-- whole ceiling at open -- the address-space budget that report 02 counts
-- against LMDB does not apply to this setting.
PRAGMA wal_autocheckpoint = 1000;
```

The content store's durability barrier is `PRAGMA wal_checkpoint(TRUNCATE)`,
issued once before the commitment transaction — never per batch. Ordering is
non-negotiable: content durable *before* the commitment that references it.

The provider also needs one store that belongs to no bucket —
`{storage_path}/provider.sqlite`, holding the negotiation nonce high-water mark
(today `CF_METADATA`/`KEY_NONCE`) and any future provider-global state. Same
PRAGMAs as the commitment store: it is small, rarely written, and must not be
lost.

Schema — one keyspace per store, no rowid overhead:

```sql
-- commitment store
CREATE TABLE IF NOT EXISTS leaves (pos INTEGER PRIMARY KEY, leaf BLOB NOT NULL) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS nodes  (pos INTEGER PRIMARY KEY, hash BLOB NOT NULL) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS meta   (k TEXT PRIMARY KEY, v BLOB NOT NULL);

-- content store
CREATE TABLE IF NOT EXISTS nodes (hash BLOB PRIMARY KEY, data BLOB NOT NULL, children BLOB) WITHOUT ROWID;
```

### Page size — 32 KiB for the content store only

At the 4 KiB default a 256 KiB chunk occupies a ~64-page overflow chain that is
walked one page at a time on read, and that chain *is* SQLite's content-store read
latency. Measured, **32 KiB is 1.58× faster on chunk reads** (452 → 286 µs warm
p50, three replicates each, disjoint ranges) — see
[report 01](01-storage-provider-benchmark.md#follow-up-is-sqlites-chunk-read-weakness-just-an-untuned-default).

Three things to know before copying that number:

- **It applies to the content store only.** A 48-byte commitment row fits any
  page size, so a larger page buys it nothing and costs it reopen latency
  (71.5 → 80.2 µs) and an 8× larger empty-bucket floor. Leave the commitment
  store at the 4 KiB default.
- **Bigger is not better past 32 KiB.** 64 KiB is slower on reads *and* worse on
  every cost metric. The optimum is interior; do not set the maximum.
- **`page_size` must be applied before `journal_mode = WAL`.** SQLite will not
  change the page size of a database already in WAL mode without a `VACUUM`, so on
  an existing bucket file the change requires a rebuild, and on a new one the
  PRAGMA order is load-bearing rather than stylistic.

The cost is an 8× larger floor for an empty content store (8 → 64 KiB, i.e.
7.6 → 61 GiB per million buckets). That is charged per bucket rather than per
byte, so it is negligible for buckets holding chunked media and only matters if
huge numbers of near-empty buckets are provisioned. Small-value amplification is
unchanged (1.29× → 1.28×).

### LRU connection pool

The reason SQLite wins is cheap reopen and low RSS: **39 µs / ~32 KiB per
instance** on tmpfs (pass 1), **87 µs / ~72 KiB** on disk-backed storage
(pass 4). Configure the pool to exploit that, and remember every hot bucket
costs **two** connections:

- **Cap open connections** well below `ulimit -n / 6` — SQLite uses ~3 FDs per
  open DB (main file + `-wal` + `-shm`), and this design opens two stores per
  bucket. At a 65k FD limit, ~10k hot buckets fit comfortably; size the LRU cap
  to your memory budget first, FDs second.
- **Memory budget:** ≈ `2 × (cache_size + ~72 KiB)` per hot bucket. With the
  2 MiB cache above, 1000 hot buckets ≈ ~4 GiB worst case — lower `cache_size` to
  `-512` (512 KiB) if you expect many simultaneously-hot buckets. The commitment
  store's working set is small enough to stay cached permanently; the content
  store's chunk reads are inherently cold and no cache policy helps them.
- **Eviction = close.** Closing a connection releases its WAL/shm FDs and cache;
  reopen is 87 µs on disk, so aggressive eviction is cheap.
- **Checkpoint on eviction.** Run `PRAGMA wal_checkpoint(TRUNCATE)` before closing
  a bucket to keep the `-wal` file from growing unbounded across sessions.

### Bucket deletion

Delete the bucket = close both connections and `unlink` the six files
(`<bucket>.{content,commitment}.sqlite` plus each one's `-wal` and `-shm`).
Sub-millisecond for every candidate engine measured, and it reclaims 100% of
space immediately — **do not** issue `DELETE FROM kv` (141 ms and reclaims little; see
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

- [ ] `ulimit -n` raised to comfortably exceed `6 × max_open_buckets` (two SQLite stores per bucket) on the provider host.
- [ ] LRU pool cap sized against the memory budget first, FDs second.
- [ ] Scheduled compaction/vacuum job — no engine reclaims space on a bare delete.
- [ ] SQLite buckets deleted via file `unlink`, never `DELETE FROM`.
- [ ] Commitment store at `synchronous = FULL`; content store at `NORMAL` with an explicit barrier before every commitment.
- [ ] `provider.sqlite` created and carrying the nonce high-water mark (it has no per-bucket home).
