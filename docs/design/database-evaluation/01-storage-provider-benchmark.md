# Storage Provider Benchmark — Architecture × Engine

**Component:** Storage Provider.
**Source data:** six result files across five measurement passes, each **not**
record-for-record comparable with the others — every section states which it uses.
1. [`results/storage-provider.json`](results/storage-provider.json) — the original matrix (tmpfs).
2. [`results/storage-provider-compaction-run.json`](results/storage-provider-compaction-run.json) — adds redb and post-compaction disk (tmpfs; sled OOM-killed).
3. [`results/content-store-run.json`](results/content-store-run.json) — the `content_store` scenario (tmpfs).
4. [`results/per-instance-run.json`](results/per-instance-run.json) — adds LMDB/libmdbx/jammdb and the per-instance metrics, **on disk-backed storage**, all eight engines in one sweep.
5. [`results/sqlite-page-size-run.json`](results/sqlite-page-size-run.json) — the SQLite `page_size` study (sweep + replicates), SQLite only.
6. [`results/final-run.json`](results/final-run.json) — **harness v2**: retired scenarios removed, the dedup lookup measured, SQLite's durability mapping corrected. All eight engines, disk-backed.

All passes use seed `13371337`, scale `1.0`.
**Read the [methodology and caveats](README.md#methodology) first.** Passes 1–3 ran
on tmpfs, so their absolute *write* throughput is optimistic; pass 4 did not, and
it initially appeared to reverse what passes 1–3 concluded about sharded-vs-shared
concurrency — that reversal was [a harness bug and is withdrawn](#retracted-the-concurrency-inversion-was-a-harness-bug).

## Two questions, not one

Issue #100 asks an architectural question the original benchmark wrongly assumed away: should the Storage Provider use **one database per bucket (sharded)** or **one shared database for all buckets (shared)**? The engine choice depends on that answer, so this report benchmarks the full matrix:

- **Architectures:** sharded (one DB file per bucket, behind an LRU pool) vs shared (single DB, keys = `bucket_id || position`).
- **Engines:** RocksDB, Sled, SQLite, and **redb** in both; **ParityDB** added to *shared* (its per-instance overhead rules it out of sharded — see below); **LMDB, libmdbx and jammdb** added to *sharded* in the fourth pass, as the engines built to be instantiated many times over.

redb was added after the first measurement pass, together with the post-compaction disk measurements. Both come from a second run and are reported in their own sections at the end — see [the redb section](#redb--a-fifth-candidate-second-measurement-pass) for why those numbers are not merged into the tables above.

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

## redb — a fifth candidate (second measurement pass)

[redb](https://github.com/cberner/redb) is a pure-Rust, copy-on-write B-tree —
the same index family as SQLite and Sled, reached through a typed key/value API
instead of SQL. It answers a question the original candidate set left open:
**how much is SQLite's SQL layer actually costing us?** Sled was the only other
pure-KV B-tree, and it lost on grounds specific to Sled (2.5 GiB RSS, reopen
friction) rather than to the index family.

> [!IMPORTANT]
> **This section's numbers come from a separate run and cannot be interleaved
> with the tables above.** Re-running SQLite unchanged — same seed, same host,
> same code — moved its figures by 25–48% versus the first pass (RSS by 121%,
> reopen p99 by more than 10×). Every comparison below is therefore
> **fresh-vs-fresh**, drawn only from
> [`results/storage-provider-compaction-run.json`](results/storage-provider-compaction-run.json).
> That variance is itself a finding, and it is larger than the README's caveats
> imply: treat the *ranking* as the deliverable and the absolute figures as
> indicative only. Sled is absent from this pass — it was OOM-killed twice
> during the sharded `multi_instance` scenario, because its ~2.5 GiB RSS
> competes for the same RAM that the tmpfs scratch uses to hold its data files.

### Sharded, per-instance

| Criterion | SQLite | **redb** | RocksDB |
|-----------|-------:|---------:|--------:|
| Reopen p50 (µs) | 38.4 | **26.8** | 1,146.7 |
| Reopen p99 (µs) | 2,078.5 | **81.8** | 1,587.2 |
| MMR append (48 B) op/s | 666,222 | 647,898 | **2,764,808** |
| Proof read p50 (µs) | 3.17 | **0.79** | 1.21 |
| Proof read p99 (µs) | 6.21 | 2.79 | **1.88** |
| RSS @ 1000 instances | 69.9 MiB | **31.2 MiB** | 169.2 MiB |
| FDs / instance | 3 | **1** | 7 |
| Concurrent, 8 buckets (op/s) | 790,494 | 3,330,041 | **4,811,138** |

**redb beats SQLite on every metric [02-recommendations.md](02-recommendations.md)
names as a sharded cost driver** — reopen latency (1.43×), per-instance memory
(2.24×), and FD pressure (3×) — plus point-read latency (4×) and concurrent
write throughput (4.2×). Append throughput is the one near-tie, and by the
report's own reasoning it does not matter: an MMR checkpoint writes a handful of
positions.

Two structural notes. RocksDB's reopen cost stays disqualifying at ~1.1 ms
(30–40× redb), which is the number an LRU pool pays on every reload. And redb's
single-writer rule is enforced in-process on a condvar rather than by a file
lock, which shows up as the largest sharded-vs-shared gap of any engine:

| Engine | Sharded (op/s) | Shared (op/s) | Sharded advantage |
|--------|---------------:|--------------:|------------------:|
| **redb** | 3,330,041 | 480,359 | **6.93×** |
| RocksDB | 4,811,138 | 2,601,845 | 1.85× |
| SQLite | 790,494 | 592,295 | 1.33× |
| ParityDB | — | 573,744 | (sharded n/a) |

redb serializes harder than SQLite under a shared database. That is irrelevant
to the sharded recommendation, but it rules redb out of any shared design.

### The disqualifier: disk amplification

| Engine | 48 B, as written | 48 B, after compaction | 256 KiB, as written | 256 KiB, compacted |
|--------|-----------------:|-----------------------:|--------------------:|-------------------:|
| RocksDB | **1.10×** | **1.10×** | **1.00×** | **1.00×** |
| SQLite | 1.29× | **1.13×** | **1.00×** | **1.00×** |
| **redb** | 3.01× | **4.65×** | 2.06× | 2.00× |

redb costs 2.6–4× SQLite's disk for the small-value MMR workload — the same
order as the 3.23× that disqualified Sled. Two mechanisms explain it, and the
first is specific to *this* workload:

1. **Unconditional 50/50 page splits.** redb divides a full page in half with no
   special case for appending at the right edge
   (`tree_store/btree_base.rs:698` for leaves, `:1499` for branches). MMR leaves
   arrive in strictly increasing position order, which is the pathological input
   for that policy: every page left behind is permanently ~half full. SQLite has
   an explicit optimization for exactly this case — `balanceQuick()` in
   `btree.c` allocates a fresh page instead of splitting when the insert lands
   on the right-most leaf, so sequential inserts pack pages to near-full.
2. **Copy-on-write shadow pages.** Every redb commit rewrites the whole
   root-to-leaf path into new pages; the superseded ones go to a free list.
   SQLite updates in place under its WAL.

RocksDB wins outright for a third reason: an LSM never splits pages at all.
SSTables are built by sorting and packing blocks to a target size, and
sequential integer keys prefix-compress well.

**`compact()` makes redb worse, not better.** This was measured expecting the
opposite — that redb's uncompacted 3.01× was an upper bound. It is a *lower*
bound: a single `Database::compact()` grows the small-value database from
16.8 MB to 26.1 MB (+55%), reproducibly, and identically whether compaction is
called once or driven to a fixed point. The mechanism is unconfirmed — plausibly
the allocator needs headroom to relocate pages and never truncates back — so
this is reported as measured behaviour, not as a diagnosis. Large values behave
normally (2.06× → 2.00×).

### Reading redb

A genuine contender that loses on one axis. It would be the right choice if
per-instance footprint or read tail latency were the binding constraint — it
halves memory, thirds the FDs, and quarters read latency versus SQLite. It is
the wrong choice while **disk is the constraint**, which for a storage provider
paying for capacity it is. SQLite's SQL layer turns out to cost approximately
nothing on this workload, while its 30-year-old sequential-insert optimization
is worth 2.6×.

## Deletion, revisited: compaction reclaims what tombstones defer

The second pass also added a compaction step to the deletion scenario, which
qualifies [Decisive metric 2](#decisive-metric-2--bucket-deletion-favors-sharded-decisively)
above. Deleting all 50 k keys, then compacting:

| Engine | Delete all keys | Disk after delete | Compaction | Disk after compaction | `rmtree` |
|--------|----------------:|------------------:|-----------:|----------------------:|---------:|
| RocksDB | 45.2 ms | 3.57 MiB | 5.5 ms | **0.04 MiB** | 0.126 ms |
| SQLite | 233.6 ms | 3.47 MiB | 0.3 ms | **0.01 MiB** | 0.105 ms |
| redb | 688.9 ms | 0.09 MiB | 0.1 ms | 0.04 MiB | 0.203 ms |

**The space *is* reclaimable — the original "space not reclaimed" finding was
measuring the absence of a compaction step, not a permanent cost.** Every engine
with a compaction API returns to ~0 on demand.

This sharpens rather than overturns the sharded argument. The accurate framing
is that in-place deletion **defers** reclamation to a scheduled, out-of-band
compaction, whereas `unlink` reclaims synchronously and in ~0.1 ms. What the
sharded model eliminates is the *latency spike and the operational burden of
scheduling compaction at all* — not an unrecoverable leak. Both ParityDB (no
synchronous compaction API; background reclamation only) and Sled (no manual
compaction) remain exceptions that cannot be scheduled this way.

## The content store, measured (third pass)

The two-database-per-bucket design in
[05-per-bucket-store-design.md](05-per-bucket-store-design.md) splits each
bucket into a *commitment store* (48 B MMR entries, position keys, fully
durable — modelled by `mmr_append_small` and `proof_read` above) and a
*content store* (256 KiB chunks, **content-hash keys, so random insertion
order**, unsynced ingest + one flush barrier). No earlier scenario modelled the
content store: every prior scenario uses sequential keys, and
`node_append_large` fsyncs every batch, which the barrier design deliberately
avoids. The `content_store` scenario closes that gap; results from
[`results/content-store-run.json`](results/content-store-run.json)
(same-pass, same-host — not comparable to other passes):

| Criterion | SQLite | redb | RocksDB |
|-----------|-------:|-----:|--------:|
| Ingest (unsynced, MiB/s) | 342 | 461 | **512** |
| Flush barrier (ms) | **0.1** | 0.0 | 1.7 |
| Chunk read, cold p50 / p99 (µs) | 309 / 394 | **127 / 191** | 94 / 659 |
| Chunk read, warm p50 / p99 (µs) | 317 / 453 | **9 / 311** | 76 / 197 |
| Disk amplification | **1.00×** | 2.06× | **1.00×** |

Three findings, one of them a correction:

1. **redb's 2× space cost on large values survives random keys — so it is
   *not* explained by the 50/50 split policy.** The split-policy mechanism
   (§ the disqualifier above) was verified for the 48 B sequential case, and
   random keys were expected to neutralise it; they did not move the 2.06×.
   For 256 KiB values the overhead must lie in copy-on-write shadow pages
   and/or large-value allocation granularity. The 2× verdict on the content
   store stands; the earlier mechanism claim applies only to the small-value
   case.
2. **SQLite is the slowest chunk *reader* by a wide margin** (317 µs warm p50
   vs redb 9 µs, RocksDB 76 µs). A 256 KiB row lands in a ~64-page overflow
   chain that is walked page by page on read. Sequential-insert packing —
   SQLite's decisive advantage in the small-value store — is irrelevant here,
   and the overflow chain becomes the cost instead. (Larger `page_size`
   shortens the chain ~4×; see the configuration guide.) At ~317 µs per chunk
   this is still sub-millisecond serving, but a single-threaded 1 GB
   reassembly pays ~1.3 s in DB reads vs RocksDB's ~0.3 s.
3. **The flush barrier is cheap for every engine** — but this is the metric
   tmpfs distorts most (an fsync against RAM); treat the *relative* barrier
   cost as meaningless until the SSD re-run. The read *ranking* is also less
   tmpfs-robust here than for small values: SQLite's overflow-chain walk is
   ~64 dependent page touches that tmpfs serves from RAM; on a cold SSD those
   become real I/Os, so its cold-read gap likely *widens* on real hardware.

**Reading for the content store:** disk amplification is the cost a
capacity-priced provider actually pays, and SQLite and RocksDB tie at 1.00×
while redb doubles it. Within the 1.00× pair, SQLite keeps the per-instance
profile that wins the sharded model (reopen, FDs) and RocksDB wins serving
latency at the price of ~1.1 ms reopens, 7 FDs/instance, and LSM compaction
churn. The engine choice per store is made in
[02-recommendations.md](02-recommendations.md).

---

## Cheap-to-multiply engines, and the metrics that were missing (fourth pass)

The sharded model asks one question of an engine that the earlier passes never
measured directly: *what does it cost to have a thousand of these open at once?*
This pass adds the three engines built for that — **LMDB** (via `heed`),
**libmdbx**, and **jammdb**, all mmap'd single-file B+trees — and the three
per-instance costs the earlier reports omitted: **OS threads**, **virtual
address space**, and the **on-disk floor of an empty bucket**.

> [!IMPORTANT]
> **This pass ran on a disk-backed filesystem, not tmpfs.** Passes 1–3 used a
> RAM-backed `/tmp`, which the [caveats](README.md#fairness-and-validity-caveats-read-before-trusting-absolute-numbers)
> flag as making every write number optimistic. It also caused two concrete
> failures: `ENOSPC` at 3.9 GiB, and the sled OOM that voided it in passes 2–3
> (tmpfs held the data in the same RAM sled needed). Moving scratch to disk fixed
> both — **sled completes here for the first time.** The consequence is that these
> figures are *not* comparable record-for-record with passes 1–3, and every write
> number below is far lower because `fsync` now costs something. In exchange this
> is the first pass covering **all eight engines in one internally consistent
> sweep**, so cross-engine comparison within this section is sound.

### New metric 1 — threads per instance

| Engine | 100 inst | 500 inst | 1000 inst | Threads per instance |
|--------|---------:|---------:|----------:|---------------------:|
| SQLite, redb, sled, jammdb, LMDB | 0 | 0 | 0 | **0.00** |
| RocksDB | 1 | 1 | 1 | **~0.00** (fixed pool) |
| libmdbx | 100 | 500 | **1000** | **1.00** |

Two results, one of which corrects an assumption this evaluation had been
carrying:

- **libmdbx spawns one transaction-manager thread per environment**, exactly 1:1.
  At a thousand open buckets that is a thousand threads. It was added *because*
  it removes LMDB's two liabilities (no `map_size` ceiling, unconditional
  `MDBX_NOTLS`), and this single property outweighs both. It is the measured
  counter-example, not a candidate.
- **RocksDB is exonerated on this axis.** Its background compaction threads come
  from a *fixed pool* shared across instances — one thread whether 100 or 1000
  are open. The "LSM engines spawn threads per instance" intuition is wrong for
  RocksDB, and this metric is what shows it.

### New metric 2 — virtual address space per instance

| Engine | 100 inst | 500 inst | 1000 inst |
|--------|---------:|---------:|----------:|
| LMDB (`map_size` = 1 GiB) | 100.3 GiB | 501.2 GiB | **1001.5 GiB** |
| libmdbx | 1205 GiB | 6001 GiB | **12002 GiB** |
| sled | 0.6 GiB | 2.6 GiB | 3.3 GiB |
| SQLite, redb, RocksDB, jammdb | ~0 | ~0.2 GiB | ~0.2 GiB |

LMDB's reservation is **exactly linear in `map_size` × open instances** — it maps
the whole ceiling at open. That couples two numbers that are otherwise
independent: the per-bucket size limit and the LRU pool cap. The product must fit
the process's address space (~128 TiB on x86-64, 256 TiB on aarch64).

This is the constraint that matters for buckets holding chunked media. At the
1 GiB ceiling benchmarked here, 1000 open buckets cost 1 TiB of address space and
work fine. But 1 GiB is not a realistic ceiling for a bucket of videos, and
`MDB_MAP_FULL` is what you get for guessing low. At a 256 GiB ceiling the same
pool would demand 256 TiB — past what 48-bit addressing allows. **So LMDB does
not forbid large buckets, it forbids `map_size × pool_cap` above a few hundred
TiB**, and the pool cap is the knob that has to give. libmdbx picks its own
generous geometry (~12 GiB/instance) when `max_size` is `None`, so "no ceiling to
choose" becomes "a large ceiling chosen for you".

### New metric 3 — the floor for a bucket holding nothing

Every provisioned bucket pays this before storing a byte.

| Engine | Allocated | Apparent | Files | At 1M buckets |
|--------|----------:|---------:|------:|--------------:|
| SQLite | **8 KiB** | 8 KiB | 1 | **7.6 GiB** |
| sled | **8 KiB** | 0.2 KiB | 2 | **7.6 GiB** |
| LMDB | 12 KiB | 16 KiB | 2 | 11.4 GiB |
| redb | 32 KiB | 1032 KiB | 1 | 30.5 GiB |
| RocksDB | 44 KiB | 29 KiB | 7 | 42.0 GiB |
| jammdb | 128 KiB | 128 KiB | 1 | 122.1 GiB |
| libmdbx | 256 KiB | 256 KiB | 2 | 244.1 GiB |

The allocated/apparent split is why this pass added a sparse-aware measurement:
redb's file *length* is 1 MiB while its allocated blocks are 32 KiB, and reading
the apparent number would have overstated it 32×. All disk figures in this
section are allocated blocks.

### Commitment store — 48 B values, position keys

| Engine | Reopen p50 / p99 (µs) | Append op/s | Proof read p50 / p99 (µs) | Disk amp | After compact |
|--------|----------------------:|------------:|--------------------------:|---------:|--------------:|
| **LMDB** | **45.2** / 838.2 | **60,230** | **0.46 / 0.71** | 1.23× | 1.23× |
| SQLite | 87.0 / **280.8** | 5,704 | 4.00 / 12.00 | 1.29× | **1.13×** |
| redb | 290.9 / 12124.7 | 34,998 | 0.92 / 5.62 | **1.19×** | 1.19× |
| RocksDB | 4980.5 / 7166.9 | 23,055 | 1.71 / 7.25 | **1.10×** | 1.10× |
| jammdb | 173.0 / 11192.4 | 24,462 | 1.04 / 1.58 | **4.52×** | 4.52× |
| sled | 1181.5 / 5242.4 | **87,723** | 0.71 / 9.58 | 3.14× | 3.14× |
| libmdbx | 814.5 / 40754.3 | 31,550 | 0.50 / 1.21 | 3.00× | 3.00× |

### Content store — 256 KiB chunks, content-hash keys

| Engine | Ingest MiB/s | Barrier (ms) | Cold p50 / p99 (µs) | Warm p50 / p99 (µs) | Disk amp |
|--------|-------------:|-------------:|--------------------:|--------------------:|---------:|
| **LMDB** | 536 | 197.3 | **35 / 61** | 19 / 48 | 1.02× |
| libmdbx | **543** | 232.7 | 38 / 65 | **18** / 51 | 1.06× |
| redb | 420 | **1720.1** | 336 / 876 | 26 / 676 | 2.00× |
| RocksDB | 265 | 16.3 | 174 / 2613 | 159 / 540 | **1.00×** |
| sled | 180 | **0.1** | 20 / 9487 | 17 / **36** | 2.51× |
| jammdb | 132 | 0.0 | 121 / 166 | 120 / 158 | 1.10× |
| SQLite | 115 | 0.3 | 680 / 966 | 656 / 1150 | **1.00×** |

Barrier costs are **not** comparable across engines: SQLite at
`synchronous = NORMAL` defers to the OS rather than fsyncing, so its 0.3 ms is
not the same act as LMDB's 197 ms genuine fsync. Compare end-to-end instead —
500 MiB ingested plus the barrier: **LMDB ≈ 1.13 s, SQLite ≈ 4.36 s.** LMDB still
wins by ~3.9× while paying for real durability.

### Deletion

| Engine | Delete all keys (ms) | Retained after | Compact API | After compact | `rmtree` (ms) |
|--------|---------------------:|---------------:|:-----------:|--------------:|--------------:|
| RocksDB | **72.5** | 3.6 MiB | yes | **0.0 MiB** | 0.27 |
| sled | 134.5 | 11.1 MiB | no | 11.1 MiB | 0.86 |
| LMDB | 285.3 | 3.3 MiB | **no** | 3.3 MiB | **0.23** |
| SQLite | 616.6 | 3.5 MiB | yes | **0.0 MiB** | 0.33 |
| libmdbx | 5687.9 | **1232 MiB** | no | 1232 MiB | 0.46 |
| redb | 16032.9 | 4.0 MiB | yes | **0.0 MiB** | 0.23 |
| jammdb | 16333.8 | 16.1 MiB | no | 16.1 MiB | 0.93 |

`rmtree` — the sharded model's actual deletion path — is sub-millisecond for
every engine, so **deletion does not discriminate between sharded candidates**;
it only discriminates sharded from shared. Two notes: libmdbx's advertised
auto-shrink did not merely fail to reclaim, it left **1.2 GiB** behind, the same
failure mode ParityDB showed in the shared model. And LMDB has no compaction API
at all — irrelevant when buckets are deleted by `unlink`, decisive if they are
ever cleared in place.

### Follow-up: is SQLite's chunk-read weakness just an untuned default?

The result above that most argues for changing engines — SQLite's chunk reads
being an order of magnitude behind — was measured at SQLite's **default 4 KiB
page size**, which the harness never overrode. A 256 KiB chunk therefore occupies
a ~64-page overflow chain walked page by page. Before treating that as an engine
verdict it has to be treated as a tuning question, so `--sqlite-page-size` makes
it a variable. Source:
[`results/sqlite-page-size-run.json`](results/sqlite-page-size-run.json).

| `page_size` | Chunk read warm p50 / p99 (µs) | Ingest MiB/s | Empty floor | GiB @ 1M buckets | 48 B disk amp | Reopen p50 (µs) |
|------------:|-------------------------------:|-------------:|------------:|-----------------:|--------------:|----------------:|
| 4,096 (default) | 440 / 1882 | 157 | **8 KiB** | **7.6** | 1.29× | **71.5** |
| 8,192 | 303 / 389 | 166 | 16 KiB | 15.3 | **1.27×** | 76.4 |
| 16,384 | 266 / 414 | 181 | 32 KiB | 30.5 | **1.27×** | 76.8 |
| **32,768** | **233 / 443** | **193** | 64 KiB | 61.0 | 1.28× | 80.2 |
| 65,536 | 293 / 527 | 177 | 128 KiB | 122.1 | 1.29× | 103.1 |

**The optimum is interior, not maximal.** 64 KiB is worse than 32 KiB on reads
*and* worse on every cost column — the shorter overflow chain stops paying once
the per-page cost dominates. "Set it as large as possible" would have been the
wrong call.

Because the five points above come from single runs, the headline comparison was
replicated three times each at 4 KiB and 32 KiB:

| `page_size` | run 1 | run 2 | run 3 | Median |
|------------:|------:|------:|------:|-------:|
| 4,096 | 452.8 | 430.7 | 452.5 | **452.5 µs** |
| 32,768 | 286.5 | 280.0 | 297.2 | **286.5 µs** |

**1.58× faster, with disjoint ranges** (cold reads agree at 1.62×). Ingest
throughput does *not* reliably improve — those ranges overlap.

> [!NOTE]
> **Context matters more than run-to-run noise here.** Within one context the
> spread is ±3%, but the *same* default config reads ~452 µs standalone and
> ~656 µs inside the eight-engine sweep. Only ratios measured within a single
> context are trustworthy; absolute latencies across passes are not.

**What it costs, and what it does not.** The 8× larger empty-bucket floor
(7.6 → 61 GiB per million buckets) is the only material price, and it is charged
per *bucket*, not per byte — negligible for buckets holding chunked media,
significant only if a million near-empty buckets are provisioned. Small-value
amplification is unchanged (1.29× → 1.28×), ingest is flat-to-better, and reopen
degrades 71.5 → 80.2 µs.

**What it means for the engine question.** Tuning recovers roughly a third of the
penalty but does not close it:

| Configuration | DB reads per GiB reassembled | Share of an 8.6 s 1 Gbps transfer |
|---------------|-----------------------------:|----------------------------------:|
| SQLite, 4 KiB pages | 1.85 s | 21.5% |
| SQLite, 32 KiB pages | 1.17 s | 13.6% |
| LMDB | **0.08 s** | **0.9%** |

So LMDB's read advantage **survives tuning**, narrowing from ~24× to ~15×. The
honest conclusion is that `page_size` is worth setting on its own merits — a free
1.58× on the serving path — and that it does not dissolve the case for LMDB.
Whether the remaining gap justifies an engine change is a reliability question,
not a performance one, and two cheaper mitigations come first: chunk reassembly
is parallelisable across WAL readers (the figures above are single-threaded), and
the design question of whether 256 KiB payloads belong in a KV store at all —
rather than in content-addressed files with the database holding only metadata —
is open in [05-per-bucket-store-design.md](05-per-bucket-store-design.md).

### Retracted: the "concurrency inversion" was a harness bug

Pass 4 reported that shared SQLite writes **24× faster** than sharded, inverting
[decisive metric 3](#decisive-metric-3--concurrent-multi-bucket-writes-favors-sharded)
and prompting the claim that real `fsync` costs had overturned one of the pillars
of the sharded recommendation. **That result was an artifact and is withdrawn.**

The two architectures were not paying the same durability. Both `write_kv` and
`write_bucket` request `sync = true`, but the two SQLite code paths honoured it
differently:

- **sharded** → `SqliteStore::commit_batch` ran `PRAGMA wal_checkpoint(TRUNCATE)`
  after *every 16-row batch* — an fsync plus folding the entire WAL back into the
  main database file;
- **shared** → `SqliteWriter::commit_batch` took the flag as `_sync` and **ignored
  it**, committing at `synchronous = NORMAL`, which in WAL mode does not fsync at
  all.

So the measurement compared one architecture doing a full checkpoint per batch
against another doing no durable work whatsoever. The gap was the durability
asymmetry, not group commit.

Both paths now map a `sync` batch to a commit at `synchronous = FULL` — the
setting the commitment store actually runs in production — and `flush()` alone
performs the checkpoint. Re-measured, **seven paired replicates**, same host,
disk-backed scratch:

| | Median op/s | Min | Max |
|---|---:|---:|---:|
| Sharded | **109,163** | 35,100 | 115,886 |
| Shared | 68,243 | 30,077 | 80,460 |

**Sharded is 1.60× faster and wins 6 of the 7 paired runs.** Ranges overlap —
one slow run in each arm — so the paired count carries more weight here than the
spread. The direction matches passes 1–3 (1.04–1.75×), so the original conclusion
stands and the sharded recommendation keeps all three pillars.

The same fix corrects SQLite's durable-append figure, which the checkpoint
mapping had been suppressing by more than an order of magnitude:

| Scenario | Pass 4 (checkpoint per batch) | Pass 5 (`synchronous = FULL`) |
|---|---:|---:|
| `mmr_append_small` | 5,704 op/s | **45,634 op/s** (8.0×) |

Both figures come from the eight-engine sweep, so the ratio is context-matched;
standalone SQLite runs put the corrected rate at 96,813 op/s, the same
context-dependence that separates a 452 µs chunk read standalone from 656 µs
inside the sweep.

Every SQLite *write* number from passes 1–4 is therefore understated. Read,
space, reopen and per-instance figures are unaffected — they never touched the
`sync` path.

> [!NOTE]
> The lesson is narrower than "tmpfs misleads" and sharper: a cross-architecture
> comparison is only meaningful if both arms are doing the same work. The pass-4
> result was reproducible across three runs and mechanically explicable — group
> commit *is* a real effect — which is exactly why it survived scrutiny until the
> two code paths were read side by side.

### Reading the fourth pass

Source: [`results/per-instance-run.json`](results/per-instance-run.json).

**LMDB beats SQLite on every sharded cost driver measured here, at essentially
equal space.** Reopen 1.9× faster (45 vs 87 µs), append 10×, proof reads 8.7× at
p50 and **17× at p99**, chunk reads **35×** (19 vs 656 µs — narrowing to ~15× once SQLite's `page_size` is
tuned, [see the follow-up](#follow-up-is-sqlites-chunk-read-weakness-just-an-untuned-default)), sharded concurrency
4.5×, and 1.02× vs 1.00× disk amplification — a 2% difference, where redb's
disqualifier was 200%. It spawns no threads, uses 3 FDs, and has the second
lowest empty-bucket floor. This is the first candidate to threaten SQLite's
position on the merits rather than on a single axis.

Its costs are specific and must be weighed, not waved through:

1. **`map_size × pool_cap` is an address-space budget** that no other engine
   imposes, and buckets of chunked media push `map_size` up hard.
2. **No compaction API** — fine under `unlink`-per-bucket deletion, disqualifying
   if buckets are ever cleared in place.
3. **A worse reopen tail** than SQLite (838 vs 281 µs p99) despite the better p50.
4. **C FFI**, where SQLite's is equally unavoidable and equally well-trodden.
5. **Its write figures are a best case.** The harness opens LMDB
   `NO_SYNC | NO_META_SYNC` so that the `sync` flag means something. LMDB's docs
   state that `metasync=False` alone preserves integrity (losing at most the last
   transaction), but `sync=False` *"can corrupt the database or lose the last
   transactions"* — integrity holds only because `writemap=False` **and** the
   filesystem preserves write order. A production configuration would sit at a
   stronger, slower setting; SQLite's WAL at `synchronous = FULL` carries no such
   conditional. Reliability, not throughput, is why the recommendation stays with
   SQLite — see [Why not LMDB (yet)](02-recommendations.md#why-not-lmdb-yet).

jammdb answers the question it was added for — whether the profile comes from the
mmap'd-B+tree design or from LMDB's specific implementation — and the answer is
*the implementation*: same design, **4.52× disk amplification**, the same order
that disqualified sled. libmdbx is disqualified twice over, on threads and on the
1.2 GiB it refuses to give back.

**These results do not by themselves change the recommendation in
[02-recommendations.md](02-recommendations.md)** — one host, one virtualized
filesystem, and a medium change that severs comparison with the earlier passes.
What they do is make LMDB the candidate to beat. The sharded-vs-shared question
is settled where passes 1–3 left it, the pass-4 inversion having been
[withdrawn](#retracted-the-concurrency-inversion-was-a-harness-bug).

---

## Harness v2 and the dedup lookup (fifth pass)

Pass 5 re-ran the full matrix after three changes that make the harness model the
[two-store design](05-per-bucket-store-design.md) rather than an approximation of
it. Source: [`results/final-run.json`](results/final-run.json), all eight engines,
disk-backed scratch, one process per engine.

1. **Retired `node_append_large` and `disk_large`** — 256 KiB values under
   *sequential position keys with a per-batch fsync*. No such load exists in the
   design: chunks are hash-keyed and barrier-durable, position-keyed values are
   48 bytes. `content_store` covers the real large-value path.
2. **SQLite's `sync` batch now means `synchronous = FULL`**, not a full
   `wal_checkpoint(TRUNCATE)` per batch — see the retraction above.
3. **Added the dedup lookup** that every upload performs before storing a chunk.

### The dedup lookup is the sharpest engine discriminator yet found

Before writing a chunk the provider checks whether that content hash is already
present. For new content the lookup **misses**, and absent-key cost is not the
same as present-key cost — an LSM can answer from a bloom filter without touching
a table, a B-tree still descends. Measured during ingest, which is when it
actually happens:

| Engine | Dedup miss p50 / p99 (µs) | Chunk read warm p50 (µs) | Ingest MiB/s |
|--------|--------------------------:|-------------------------:|-------------:|
| **LMDB** | **0.50 / 3.33** | **19** | **715** |
| sled | 1.54 / 6850.58 | 25 | 277 |
| libmdbx | 1.58 / 7.79 | 18 | 582 |
| redb | 2.50 / 365.79 | 14 | 295 |
| jammdb | 57.33 / 123.42 | 115 | 124 |
| RocksDB | 595.12 / 1923.79 | 219 | 282 |
| **SQLite** | **746.04 / 1200.67** | 711 | 115 |

**SQLite's absent-key lookup costs more than its present-key read** (746 vs
711 µs), and roughly 1500× more than LMDB's. Pass 5 attributed this to the
un-checkpointed WAL that bulk ingest accumulates. **That was wrong**, and the
experiment that disproved it is below.

### The dedup experiment: three hypotheses, one cause, one fix

Source: [`results/dedup-experiment.json`](results/dedup-experiment.json). Four
controlled runs, each designed so that a negative result would be unambiguous.

**1. Not the write-ahead log.** Checkpointing every 8 batches holds SQLite's WAL
at 0 bytes for the whole ingest. The miss still costs 393–456 µs, against 2.29 µs
on an empty store. RocksDB behaves the same way — WAL at 0, misses still
429–1170 µs. Checkpoint policy is worth ~1.6×, not the gap. **Hypothesis
withdrawn.**

**2. Not the page cache — but mmap matters.** Raising `cache_size` to 64 MiB
barely moves it (809 → 731 µs). Enabling `mmap_size` gives **4.3×**
(809 → 189 µs), and adding the larger cache on top of mmap changes nothing
further. Real, worth having, and still only 4.3× of a ~1900× gap. **Not the
cause either.**

**3. The cause: payload volume inside the key's B-tree.** Holding the key count
fixed and shrinking values isolates it:

| Shape | SQLite miss p50 | LMDB miss p50 |
|-------|----------------:|--------------:|
| 2,000 keys × 256 KiB | **200.00 µs** | 0.46 µs |
| 2,000 keys × 48 B | **2.62 µs** | 0.29 µs |
| 100,000 keys × 48 B | **3.50 µs** | 0.50 µs |

Same keys with small values is **76× faster**; **fifty times more keys costs
1.3×**. The cost follows *bytes stored*, not cardinality. The harness schema —
`kv(k BLOB PRIMARY KEY, v BLOB) WITHOUT ROWID` — stores the row **in** the index,
so a key search descends pages packed with 256 KiB chunk payloads. LMDB keeps
large values in overflow pages a key search never touches, which is why it is flat
across all three shapes.

**4. The fix, measured rather than inferred.** Give the hash its own index over a
rowid table (`CREATE TABLE kv (id INTEGER PRIMARY KEY, k BLOB, v BLOB)` plus
`CREATE UNIQUE INDEX kv_k ON kv (k)`), so the index holds `(hash, rowid)` and no
payload. Same run, same host, `--sqlite-split-index`:

| `content_store`, mmap on | `WITHOUT ROWID` | Split index | Change |
|---|---:|---:|---|
| Dedup miss p50 | 197.42 µs | **3.92 µs** | **50× faster** |
| Chunk read warm p50 | 166.12 µs | **34.58 µs** | **4.8× faster** |
| Chunk read cold p50 | 235.00 µs | **54.33 µs** | 4.3× faster |
| Ingest | 175 MiB/s | **205 MiB/s** | 17% faster |
| Disk amplification | 1.00× | 1.00× | unchanged |

The split index was expected to *cost* something on the hit path — one extra hop
from index to row. It does the opposite: the index B-tree is now small enough to
stay cached, so hits get 4.8× faster too, at identical space.

**What this does to the engine comparison.** Against pass 5's untuned SQLite:

| Metric | SQLite as measured | SQLite tuned (mmap + split) | LMDB | Remaining gap |
|---|---:|---:|---:|---:|
| Dedup miss p50 | 746 µs | **3.92 µs** | 0.50 µs | 7.8× |
| Chunk read warm p50 | 711 µs | **34.58 µs** | 19 µs | 1.8× |

**The 1500× dedup gap and the 35× chunk-read gap were configuration and schema
artifacts, not engine properties.** Tuned, SQLite is within 1.8× of LMDB on chunk
serving and 7.8× on dedup — differences that no longer come close to outweighing
the reliability argument in
[Why not LMDB (yet)](02-recommendations.md#why-not-lmdb-yet).

> [!IMPORTANT]
> Three hypotheses were advanced here and two were wrong, both of them plausible
> and both stated before being tested. The pattern is worth naming: an engine
> comparison measures the engine *plus its configuration and schema*, and a gap
> of three orders of magnitude is far more likely to be the second than the first.
> Every cross-engine number in passes 1–5 was taken at the harness's default
> schema and mmap setting, so **SQLite's read figures throughout the earlier
> passes are understated**.

### Everything else held

| Engine | Reopen p50 (µs) | Durable append op/s | Proof read p50 (µs) | 48 B disk amp |
|--------|----------------:|--------------------:|--------------------:|--------------:|
| **LMDB** | **35.6** | 53,029 | **0.46** | 1.23× |
| SQLite | 73.3 | 45,634 | 3.75 | 1.29× |
| redb | 263.9 | 34,555 | 0.83 | 1.19× |
| jammdb | 135.4 | 27,355 | 1.04 | **4.52×** |
| sled | 1224.7 | **88,760** | 0.79 | 3.14× |
| libmdbx | 563.5 | 41,544 | 0.46 | 3.00× |
| RocksDB | 4884.4 | 24,674 | 1.54 | **1.10×** |

Pass-4 rankings survive: LMDB leads the sharded drivers, jammdb and libmdbx stay
disqualified on space, RocksDB's reopen still rules it out of an LRU pool. The
one number that moved materially is SQLite's durable append (8× faster with the
corrected mapping), which narrows but does not close its gap to LMDB.

Concurrency, now with every engine paying equal durability:

| Engine | Sharded op/s | Shared op/s |
|--------|-------------:|------------:|
| SQLite | **35,860** | 29,127 |
| redb | **43,270** | 20,876 |
| RocksDB | 126,479 | **146,785** |
| sled | 87,275 | 85,245 |
| LMDB | 82,483 | — (sharded-only candidate) |

SQLite and redb favour sharded, RocksDB mildly favours shared, sled is a tie —
the same picture as passes 1–3. LMDB's sharded figure (82,483) is 2.3× SQLite's,
not the 4.5× pass 4 reported, because pass 4's SQLite number was depressed by the
checkpoint bug.

---

## Reading

- **If sharded:** **LMDB leads on performance; SQLite is kept on reliability.** Through pass 3 the answer was SQLite on space, not speed: redb beat it on every other sharded cost driver (reopen 1.43×, RSS 2.24×, FDs 3×, read latency 4×, concurrency 4.2×) and was disqualified only by **3.01× disk amplification that compaction worsens to 4.65×**, the same order that ruled out Sled — leaving SQLite's sequential-insert page packing, which suits an append-only MMR exactly, at approximately zero cost for the SQL layer. Pass 4 changes the picture: **LMDB matches SQLite's space (1.02× vs 1.00×) while beating it on reopen, append, proof reads, chunk reads by 35×, and sharded concurrency** — the first candidate to lead on the merits rather than on one axis. Pass 5 confirms it with the harness fixed, and adds the largest gap yet — the **dedup lookup**, 0.50 µs against SQLite's 746 µs. Its own costs are the `map_size × pool_cap` address-space budget, no compaction API, a worse reopen tail, and write figures measured at a durability point production would not use. The recommendation stays with SQLite for reasons set out in [Why not LMDB (yet)](02-recommendations.md#why-not-lmdb-yet). jammdb and libmdbx are disqualified (4.52× amplification; one thread per instance plus 1.2 GiB unreclaimed). RocksDB's ~1.1 ms reopen and 7 FDs/instance still disqualify it, though pass 4 clears it of the per-instance *thread* charge.
- **If shared:** RocksDB. Best concurrent write (2.3 M op/s), tiny footprint (1.4 MiB / 7 FD), least-bad deletion (30 ms, −11%). SQLite-shared eats the single-writer penalty; ParityDB-shared has the catastrophic delete-space explosion; Sled-shared is slow.
- **Across architectures:** deletion + isolation + LRU-bounded footprint + concurrency all favour **sharded**. Pass 4 briefly reported the concurrency pillar inverted; that was [a harness bug](#retracted-the-concurrency-inversion-was-a-harness-bug) and is withdrawn — re-measured fairly, sharded leads 1.60× (6 of 7 paired runs), consistent with passes 1–3. The deletion argument is about *latency and operational burden*, not permanent space loss — compaction does reclaim the space (see [Deletion, revisited](#deletion-revisited-compaction-reclaims-what-tombstones-defer)); `unlink` simply does it synchronously in ~0.1 ms with nothing to schedule, and pass 4 shows every sharded candidate does it sub-millisecond, so deletion no longer discriminates *between* engines. Shared's memory edge is still bounded by the LRU pool regardless of bucket count.

The architecture-aware recommendation and the crossover conditions are in
[02-recommendations.md](02-recommendations.md), which is current through pass 5:
it weighs LMDB's lead explicitly in
[Why not LMDB (yet)](02-recommendations.md#why-not-lmdb-yet) and reflects the
withdrawn concurrency inversion. The standing caveat is the host, not the
analysis — every pass ran on one virtualized machine, so **re-run the suite on
representative SSD hardware before committing budget**, particularly for the read
rankings, which are the least medium-robust figures here.
