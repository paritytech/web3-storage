# Configuration Guide

Concrete tuning for the chosen engine — SQLite in WAL mode, **one database per
bucket** holding the content and commitment table groups per
[05-per-bucket-store-design.md](05-per-bucket-store-design.md) — plus the on-chain
key-layout guidance that survives independently of it.

The two table groups get **different durability, chosen per transaction on the
same connection**, and the difference is load-bearing: the commitment tables hold
the slashable state and their transactions must be fully durable, while the
content tables hold hash-verified, re-fetchable chunks and buy throughput with
relaxed per-write durability. Because both share one WAL, the commitment
transaction's fsync also pins every content write before it.

---

## Storage Provider — SQLite (WAL) per bucket

### Per-connection PRAGMAs

Apply at open (mirrors the harness in
[`benchmarks/db-bench/src/engines/sqlite.rs`](../../../benchmarks/db-bench/src/engines/sqlite.rs)):

```sql
-- One file per bucket: {storage_path}/buckets/{bucket_id}.sqlite
PRAGMA page_size    = 32768;      -- set FIRST, before WAL; see below
PRAGMA journal_mode = WAL;
PRAGMA synchronous  = NORMAL;     -- the default for content transactions; raised
                                  -- to FULL around each commitment transaction
PRAGMA busy_timeout = 5000;
PRAGMA cache_size   = -2000;      -- ~2 MiB; the commitment working set is ~10 MB per 100 k uploads
PRAGMA mmap_size    = 1073741824; -- 1 GiB: worth 4.3x on chunk reads, see below
-- NOTE: SQLite's mmap_size is a *cap*, not a reservation. It maps existing pages
-- up to that limit, so address space grows with the file and 1 GiB here costs
-- nothing on a small bucket. This is unlike LMDB's map_size, which reserves the
-- whole ceiling at open -- the address-space budget that report 02 counts
-- against LMDB does not apply to this setting.
PRAGMA wal_autocheckpoint = 1000;
```

`PRAGMA synchronous` is per connection and may be changed between transactions
(not inside one). The write sequence is therefore:

```sql
-- content: any number of batches
BEGIN; INSERT INTO chunks …; COMMIT;            -- at NORMAL: WAL append, no fsync

-- commitment: exactly one transaction per commit
PRAGMA synchronous = FULL;
BEGIN; INSERT INTO leaves …; INSERT INTO mmr_nodes …; UPDATE meta …; COMMIT;  -- fsyncs the WAL
PRAGMA synchronous = NORMAL;
```

WAL recovery is prefix-consistent, so that one fsync durably records every
content transaction that preceded it. No separate `wal_checkpoint` barrier is
needed for correctness; checkpoints are WAL-size housekeeping (see the pool
section). Ordering is still non-negotiable — content transactions *before* the
commitment that references them, and the commitment commit *before* signing.

The provider also needs one store that belongs to no bucket —
`{storage_path}/provider.sqlite`, holding the negotiation nonce high-water mark
(today `CF_METADATA`/`KEY_NONCE`) and any future provider-global state. Default
`page_size`, `synchronous = FULL`, `mmap_size = 0`: it is small, rarely written,
and must not be lost.

Schema — two table groups in one file. Payload-in-index for the 48-byte rows,
a separate hash index for the 256 KiB rows:

```sql
-- commitment store
CREATE TABLE IF NOT EXISTS leaves    (pos INTEGER PRIMARY KEY, leaf BLOB NOT NULL) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS mmr_nodes (pos INTEGER PRIMARY KEY, hash BLOB NOT NULL) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS meta      (k TEXT PRIMARY KEY, v BLOB NOT NULL);

-- content store: the hash index MUST be a separate B-tree from the payload
CREATE TABLE IF NOT EXISTS chunks (
  id       INTEGER PRIMARY KEY,
  hash     BLOB NOT NULL,
  data     BLOB NOT NULL,
  children BLOB
);
CREATE UNIQUE INDEX IF NOT EXISTS chunks_hash ON chunks (hash);
```

`hash BLOB PRIMARY KEY … WITHOUT ROWID` for the content table would put 256 KiB
payloads inside the B-tree every dedup check descends — measured **50× slower on
the dedup lookup and 4.8× slower on chunk reads**; see the
[dedup experiment](01-storage-provider-benchmark.md#the-dedup-experiment-three-hypotheses-one-cause-one-fix).

### Page size — 32 KiB for the bucket file

At the 4 KiB default a 256 KiB chunk occupies a ~64-page overflow chain that is
walked one page at a time on read, and that chain *is* SQLite's chunk-read
latency. Measured, **32 KiB is 1.58× faster on chunk reads** (452 → 286 µs warm
p50, three replicates each, disjoint ranges) — see
[report 01](01-storage-provider-benchmark.md#follow-up-is-sqlites-chunk-read-weakness-just-an-untuned-default).

`page_size` is per file, so the commitment tables share it. The same sweep ran
the commitment scenarios at each page size, and at 32 KiB versus 4 KiB they pay
little: durable 48 B appends 6,450 vs 6,956 op/s (medians of three replicates,
inside the 4 KiB replicates' own spread), proof read p50 4.9 vs 3.9 µs, reopen
unchanged, 48 B disk amplification 1.28× vs 1.29×. Three things to know before
copying that number:

- **Bigger is not better past 32 KiB.** 64 KiB is slower on reads *and* worse on
  every cost metric. The optimum is interior; do not set the maximum.
- **`page_size` must be applied before `journal_mode = WAL`.** SQLite will not
  change the page size of a database already in WAL mode without a `VACUUM`, so on
  an existing bucket file the change requires a rebuild, and on a new one the
  PRAGMA order is load-bearing rather than stylistic.
- **The cost is an 8× larger floor for an empty bucket** (8 → 64 KiB, i.e.
  7.6 → 61 GiB per million buckets). That is charged per bucket rather than per
  byte, so it is negligible for buckets holding chunked media and only matters if
  huge numbers of near-empty buckets are provisioned.

### LRU connection pool

The reason SQLite wins is cheap reopen and low RSS: **39 µs / ~32 KiB per
instance** on tmpfs (pass 1), **87 µs / ~72 KiB** on disk-backed storage
(pass 4). Configure the pool to exploit that; every hot bucket costs **one**
connection:

- **Cap open connections** well below `ulimit -n / 3` — SQLite uses ~3 FDs per
  open DB (main file + `-wal` + `-shm`). At a 65k FD limit, ~20k hot buckets fit
  comfortably; size the LRU cap to your memory budget first, FDs second.
- **Memory budget:** ≈ `cache_size + ~72 KiB` per hot bucket. With the 2 MiB
  cache above, 1000 hot buckets ≈ ~2 GiB worst case — lower `cache_size` to
  `-512` (512 KiB) if you expect many simultaneously-hot buckets. The commitment
  working set is small enough to stay cached; chunk reads are inherently cold,
  no cache policy helps them, and with `mmap_size` on they are served from the
  mapping rather than through the pager cache.
- **Eviction = close.** Closing a connection releases its WAL/shm FDs and cache;
  reopen is 87 µs on disk, so aggressive eviction is cheap.
- **Checkpoint on eviction.** Run `PRAGMA wal_checkpoint(TRUNCATE)` before closing
  a bucket to keep the `-wal` file from growing unbounded across sessions. Chunk
  ingest is what grows the WAL, so `wal_autocheckpoint` fires on content volume;
  long-running downloads (read transactions) delay WAL reset for the whole file.

### Bucket deletion

Delete the bucket = close its connection and `unlink` the three files
(`<bucket>.sqlite` plus its `-wal` and `-shm`). Sub-millisecond for every
candidate engine measured, and it reclaims 100% of space immediately — **do not**
issue `DELETE FROM …` (141 ms and reclaims little; see
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

- [ ] `ulimit -n` raised to comfortably exceed `3 × max_open_buckets` on the provider host.
- [ ] LRU pool cap sized against the memory budget first, FDs second.
- [ ] Scheduled compaction/vacuum job — no engine reclaims space on a bare delete.
- [ ] SQLite buckets deleted via file `unlink`, never `DELETE FROM`.
- [ ] Commitment transactions at `synchronous = FULL`, content transactions at `NORMAL`, on the same connection; commitment commit before signing.
- [ ] `provider.sqlite` created and carrying the nonce high-water mark (it has no per-bucket home).
