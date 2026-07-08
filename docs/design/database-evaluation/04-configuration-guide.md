# Configuration Guide

Concrete tuning for the chosen engines, plus the cross-cutting OS-level
mitigations from Issue #101 (Steps 1–4) that apply regardless of engine choice.

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

## Blockchain Node — ParityDB

### Node flags

```bash
polkadot-omni-node \
  --database paritydb \
  --state-pruning 256 \          # keep last N finalized states; archive => 'archive'
  --blocks-pruning 256 \
  --db-cache 1024                # MiB hint; tune against the cgroup cap below
  # ... existing chain/network flags
```

Validate the ranking on a node before committing (the [02](02-blockchain-provider-benchmark.md)
A/B): run the same load once with `--database rocksdb` and once with
`--database paritydb`, comparing block-import time, state-read latency, on-disk
size, and steady-state RSS.

### Memory bounding (mandatory for ParityDB)

ParityDB relies on the **OS page cache** rather than a bounded internal cache; it
peaked at 391 MiB vs RocksDB's 93 MiB under sustained load. Bound it with the
cgroup isolation in Step 2 below and size the cap to fit ParityDB's working set
(state index + hot value pages) plus the runtime and networking.

### If you fall back to RocksDB (Step 3 tuning)

Apply only if ParityDB's memory profile cannot be accommodated:

```bash
polkadot-omni-node --database rocksdb --db-cache 512
```

Compaction & buffer tuning to cushion spikes and protect SSD bandwidth (set via
the backend's RocksDB options where exposed):

- **Leveled compaction with dynamic level sizing** (`level_compaction_dynamic_level_bytes = true`) — keeps level targets proportional to data, reducing write amplification.
- **Larger write buffer** (e.g. `write_buffer_size = 64 MiB`, `max_write_buffer_number = 4`) — absorbs transaction bursts before flush.
- **Rate-limit background I/O** (`rate_limiter` ~ a fraction of device write bandwidth) — stops compaction from starving block production.
- **Bounded block cache** (`--db-cache`) — RocksDB's explicit cache is its memory advantage; keep it sized and capped.

---

## Cross-cutting mitigations (Issue #101 Steps 1–4)

These apply regardless of which engines are chosen.

### Step 1 — Physical data isolation

Keep the consensus database storing **only 32-byte hash pointers and state
metrics**, never raw file bytes. In this codebase the chain already holds only
MMR roots / commitments and the provider node holds blobs off-chain — preserve
that boundary. Put the Blockchain Provider's DB and the Storage Provider's blob
store on **separate volumes** so file-transfer I/O cannot contend with consensus
DB I/O.

### Step 2 — System memory safeguards (cgroups)

Prevent the Storage Provider's HTTP/file-sharing traffic from purging the
Blockchain Provider's (page-cache-resident) DB index out of RAM. This is
**mandatory given ParityDB's page-cache reliance** (measured 391 MiB working set).

Example systemd slices:

```ini
# blockchain-provider.service  — protect its page cache
[Service]
MemoryMin=2G            # reclaim protection: keep at least this resident
MemoryHigh=4G
MemoryMax=5G

# storage-provider.service     — cap file-transfer page-cache churn
[Service]
MemoryHigh=2G
MemoryMax=3G
```

`MemoryMin` on the blockchain service is the key line: it reserves reclaim-protected
memory so a burst of uploads on the storage service cannot evict ParityDB's hot
index pages.

### Step 3 — Compaction & buffer tuning

Covered above under the RocksDB fallback. With ParityDB chosen for the chain and
SQLite for buckets, classic LSM compaction tuning applies only if RocksDB is used
as a fallback on either side.

### Step 4 — Key-prefix restructuring

Structure on-chain storage keys so entries sharing a parent (e.g. a Bucket ID)
sort contiguously, enabling bulk range deletion in one pass instead of scattered
tombstones. Concretely, prefer composite keys `(&bucket_id, &item_id)` /
`StorageDoubleMap<BucketId, ItemId, _>` over hashing the pair into one opaque key,
so a bucket's entries form a contiguous range that `clear_prefix` can drop
efficiently. (The shared-DB benchmark uses exactly this `bucket_id || position`
layout.)

> Note: the original plan attributed this to Issue #65, but Issue #65 is the
> "Robust Syncing Protocol for Dynamic Primary and Replica Node Topologies" — a
> different topic. Track key-prefix restructuring here under Issue #101 (or a new
> dedicated issue), not under #65.

---

## OS-level checklist

- [ ] `ulimit -n` raised to comfortably exceed `3 × max_open_buckets` (SQLite) on the provider host.
- [ ] Consensus DB volume separate from blob-store volume (Step 1).
- [ ] systemd `MemoryMin` set on the blockchain service; `MemoryMax` on the storage service (Step 2).
- [ ] ParityDB `--db-cache` sized to fit within the cgroup cap.
- [ ] Scheduled pruning/compaction job (neither engine reclaims space on bare delete).
- [ ] SQLite buckets deleted via file `unlink`, never `DELETE FROM`.
