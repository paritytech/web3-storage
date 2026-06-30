# Storage Provider Benchmark — Architecture × Engine

**Component:** Storage Provider.
**Source data:** [`results/storage-provider.json`](results/storage-provider.json) (seed `13371337`, scale `1.0`).
**Read the [methodology and caveats](README.md#methodology) first** — the scratch FS is tmpfs, so absolute *write* throughput is optimistic and, importantly, the **shared-SQLite single-writer penalty is *understated*** here (see the concurrency section).

## Two questions, not one

Issue #100 asks an architectural question the original benchmark wrongly assumed away: should the Storage Provider use **one database per bucket (sharded)** or **one shared database for all buckets (shared)**? The engine choice depends on that answer, so this report benchmarks the full matrix:

- **Architectures:** sharded (one DB file per bucket, behind an LRU pool) vs shared (single DB, keys = `bucket_id || position`).
- **Engines:** RocksDB, Sled, SQLite in both; **ParityDB** added to *shared* (its per-instance overhead rules it out of sharded — see below).

## The architecture trade-off in one table

What each architecture wins, measured (1000 buckets):

| Dimension | Sharded (per-bucket file) | Shared (single DB) | Decisive for |
|-----------|---------------------------|--------------------|--------------|
| Memory at 1000 buckets | grows per instance (Sled **2.5 GiB**, RocksDB 77 MiB, SQLite 32 MiB) | flat, one instance (**0–24 MiB** all engines) | **Shared** |
| File descriptors | grows (RocksDB **7000**, SQLite 3000, Sled 1000) | flat (2–7 total) | **Shared** |
| Bucket deletion | file `unlink` **~0.1 ms, 100% reclaimed** | in-place key deletes **19–141 ms, space not reclaimed** (ParityDB *grows 8×*) | **Sharded** |
| Concurrent multi-bucket writes | parallel files | contends on one DB (SQLite single-writer) | **Sharded** (1.04–1.75×) |
| Fault isolation | one corrupt file = one bucket | one DB = blast radius is everything | **Sharded** |
| Read latency | sub-µs–2.7 µs p50 | sub-µs–2.5 µs p50 | tie |

The headline: **shared wins on resource footprint; sharded wins on deletion, concurrency, and isolation.** The LRU pool tips the balance (below).

---

## Decisive metric 1 — memory & FD scaling (favors shared)

Sharded: all live instances cost RAM and FDs simultaneously.

| Engine | RSS @100 | RSS @500 | RSS @1000 | FD/inst |
|--------|---------:|---------:|----------:|--------:|
| SQLite | 3.1 MiB | 15.6 MiB | 31.6 MiB | 3 |
| RocksDB | ~0 | 20.2 MiB | 77.4 MiB | 7 |
| Sled | 27.9 MiB | 721.9 MiB | **2560.0 MiB** | 1 |

Shared: one instance holding all 1000 buckets (64 k entries).

| Engine | RSS | FDs | on-disk (logical 3.9 MiB) |
|--------|----:|----:|--------------------------:|
| SQLite | ~0 MiB | 3 | 4.9 MiB (1.27×) |
| RocksDB | 1.4 MiB | 7 | 3.8 MiB (0.98×) |
| ParityDB | 23.6 MiB | 2 | 3.5 MiB (0.88×) |
| Sled | 5.9 MiB | 1 | 14.7 MiB (3.77×) |

**Sled is only viable shared** (5.9 MiB vs 2.5 GiB). **RocksDB's 7000 sharded FDs** breach a default 1024 limit, but shared it uses 7. This is shared's structural advantage — *but* it is largely neutralized by the LRU pool: in the sharded model only the **hot** buckets are open, so RAM/FDs are bounded by the pool cap, not the total bucket count. Cold buckets are just files. SQLite sharded already fits 1000 *simultaneously-hot* buckets in 32 MiB, so the pool rarely needs to evict.

## Decisive metric 2 — bucket deletion (favors sharded, decisively)

The issue calls out "clearing expired agreements" as a tombstone-spike bottleneck. Clearing **500 of 1000 buckets** (32 k keys):

| Engine | Shared: in-place delete | Disk before → after | Reclaimed |
|--------|------------------------:|--------------------:|----------:|
| ParityDB | 19 ms | 3.9 → **36.8 MiB** | **−835%** (exploded) |
| RocksDB | 30 ms | 3.8 → 4.2 MiB | −11% (grew) |
| Sled | 60 ms | 14.6 → 17.1 MiB | −17% (grew) |
| SQLite | 141 ms | 4.9 → 4.9 MiB | +1% (flat) |

**Sharded equivalent: a filesystem `unlink` per bucket — ~0.1 ms, 100% of space reclaimed, immediately.** In-place deletion in a shared DB is ~200–1400× slower *and* fails to reclaim space (catastrophically for ParityDB, whose append-only log balloons 8×). This is exactly the deletion-latency-spike bottleneck the issue raised, and the sharded model eliminates it.

## Decisive metric 3 — concurrent multi-bucket writes (favors sharded)

8 threads, each writing a distinct bucket, sync per batch:

| Engine | Sharded (op/s) | Shared (op/s) | Sharded advantage |
|--------|---------------:|--------------:|------------------:|
| RocksDB | 4,042,535 | 2,303,296 | **1.75×** |
| SQLite | 747,456 | 580,070 | 1.29× |
| Sled | 111,286 | 106,935 | 1.04× |
| ParityDB | — | 571,784 | (sharded n/a) |

Sharded wins for every engine because separate files don't contend. SQLite's shared penalty (1.29×) looks mild **only because tmpfs makes the write-lock hold time near-zero** — on real SSD each serialized commit waits on a real `fsync`, so a shared-SQLite design's single-writer serialization would be dramatically worse than shown. Treat 1.29× as a floor, not the expected production gap.

## Engine detail (sharded scenarios)

These are the per-bucket-instance measurements that decide *which engine* to use if sharded is chosen.

| Criterion | Sled | SQLite | RocksDB |
|-----------|-----:|-------:|--------:|
| Reopen p50 / p99 (µs) | 449.5 / 2492.6 | **39.0 / 127.2** | 1055.6 / 1425.4 |
| MMR append (48 B) op/s | 106,054 | 629,085 | **2,858,402** |
| Proof read p50 / p99 (µs) | **0.5** / 6.2 | 2.7 / 7.7 | 1.1 / **1.5** |
| Disk amplification (48 B) | 3.23× | 1.29× | **1.10×** |

- **Reopen latency** is the per-bucket cost paid on every LRU reload; SQLite is **~27× faster than RocksDB** here, the single most important number for a sharded LRU pool.
- RocksDB has the best raw append throughput and read tail, but its reopen cost and 7 FDs/instance are liabilities at scale.
- Sled's 2.5 GiB RAM and 3.2× disk amplification rule it out of the sharded model entirely.

---

## Reading

- **If sharded:** SQLite. Only engine with acceptable per-instance footprint (32 MiB / 1000), fastest reopen (39 µs), instant `unlink` deletion, and best concurrency (parallel files). RocksDB's FDs and Sled's RAM disqualify them.
- **If shared:** RocksDB. Best concurrent write (2.3 M op/s), tiny footprint (1.4 MiB / 7 FD), least-bad deletion (30 ms, −11%). SQLite-shared eats the single-writer penalty; ParityDB-shared has the catastrophic delete-space explosion; Sled-shared is slow.
- **Across architectures:** deletion + isolation + LRU-bounded footprint make **sharded + SQLite** the recommendation. Shared's only real edge — total memory — is already small for SQLite sharded and is bounded by the LRU pool regardless of bucket count.

The architecture-aware recommendation and the crossover conditions are in
[03-recommendations.md](03-recommendations.md).
