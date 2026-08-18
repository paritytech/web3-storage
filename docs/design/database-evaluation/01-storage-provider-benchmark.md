# Storage Provider Benchmark — Architecture × Engine

**Component:** Storage Provider.
**Source data:** [`results/storage-provider.json`](results/storage-provider.json) (seed `13371337`, scale `1.0`); the redb and compaction sections draw on a second pass, [`results/storage-provider-compaction-run.json`](results/storage-provider-compaction-run.json) (same seed and scale) — the two are **not** record-for-record comparable, see that section's note.
**Read the [methodology and caveats](README.md#methodology) first** — the scratch FS is tmpfs, so absolute *write* throughput is optimistic and, importantly, the **shared-SQLite single-writer penalty is *understated*** here (see the concurrency section).

## Two questions, not one

Issue #100 asks an architectural question the original benchmark wrongly assumed away: should the Storage Provider use **one database per bucket (sharded)** or **one shared database for all buckets (shared)**? The engine choice depends on that answer, so this report benchmarks the full matrix:

- **Architectures:** sharded (one DB file per bucket, behind an LRU pool) vs shared (single DB, keys = `bucket_id || position`).
- **Engines:** RocksDB, Sled, SQLite, and **redb** in both; **ParityDB** added to *shared* (its per-instance overhead rules it out of sharded — see below).

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

---

## Reading

- **If sharded:** SQLite — but on space, not speed. **redb beats it on every other sharded cost driver** (reopen 1.43×, RSS 2.24×, FDs 3×, read latency 4×, concurrency 4.2×) and is disqualified only by **3.01× disk amplification that compaction worsens to 4.65×**, the same order that ruled out Sled. RocksDB's ~1.1 ms reopen and 7 FDs/instance disqualify it. SQLite's remaining edge is that its sequential-insert page packing suits an append-only MMR exactly; its SQL layer costs approximately nothing here.
- **If shared:** RocksDB. Best concurrent write (2.3 M op/s), tiny footprint (1.4 MiB / 7 FD), least-bad deletion (30 ms, −11%). SQLite-shared eats the single-writer penalty; ParityDB-shared has the catastrophic delete-space explosion; Sled-shared is slow.
- **Across architectures:** deletion + isolation + LRU-bounded footprint make **sharded + SQLite** the recommendation. Note the deletion argument is about *latency and operational burden*, not permanent space loss — compaction does reclaim the space (see [Deletion, revisited](#deletion-revisited-compaction-reclaims-what-tombstones-defer)); `unlink` simply does it synchronously in ~0.1 ms with nothing to schedule. Shared's only real edge — total memory — is already small for SQLite sharded and is bounded by the LRU pool regardless of bucket count.

The architecture-aware recommendation and the crossover conditions are in
[02-recommendations.md](02-recommendations.md).
