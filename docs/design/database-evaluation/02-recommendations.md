# Recommendations

Backed by the measured results in [01](01-storage-provider-benchmark.md). The
recommendation states the choice, the justification from the data, the trade-off
accepted, and the conditions that would change it.

Scope: the **Storage Provider** only. The chain runs on Asset Hub, so its
database backend is not ours to choose.

---

## Storage Provider → **Sharded (one DB per bucket) + SQLite (WAL)**

This is a two-part decision: first the *architecture* (sharded vs shared),
then the *engine*. Both are backed by the architecture × engine matrix
in [01](01-storage-provider-benchmark.md).

### Decision

1. **Architecture: sharded** — one database file per bucket, behind an LRU
   connection pool.
2. **Engine: SQLite (WAL mode)** within that sharded model.

### Why sharded over shared

The two architectures genuinely trade off, and we measured both:

| What it wins | Architecture | Measured |
|--------------|--------------|----------|
| Resource footprint | **Shared** | one flat instance, 1–34 MiB / 2–7 FDs for 1000 buckets vs sharded's per-instance growth |
| Bucket deletion | **Sharded** | `unlink` ~0.1 ms, 100% reclaim vs shared in-place delete 19–141 ms with **no reclaim** (ParityDB *grew 8×*) |
| Concurrent writes | **Sharded** | 1.04–1.75× faster (parallel files; no shared write lock) |
| Fault isolation | **Sharded** | one corrupt file = one bucket, not the whole store |

Sharded wins on the operations tied to the **bucket lifecycle** — creation,
expiry, deletion — which the issue explicitly flags as a tombstone-spike
bottleneck. The shared model's only real advantage is total memory, and that is
**neutralized by the LRU pool**: in the sharded model only *hot* buckets are
open, so RAM/FDs are bounded by the pool cap, not the bucket count, while cold
buckets are just cheap files. The deletion, concurrency, and isolation wins are
not recoverable in the shared model. Hence sharded.

### Why SQLite within the sharded model

The sharded cost drivers are reopen latency, per-instance memory, and FD
pressure — not raw throughput. SQLite wins or ties every one:

| Driver | SQLite | vs. alternatives | Why it matters |
|--------|--------|------------------|----------------|
| Reopen latency | **39.0 µs** p50 | 11× faster than Sled, **27× faster than RocksDB** | Paid on every LRU reload |
| RSS @ 1000 instances | **31.6 MiB** | RocksDB 77 MiB; **Sled 2.56 GiB** | Determines how many buckets fit in RAM |
| FDs / instance | 3 | Sled 1, RocksDB 7 | RocksDB's 7000 FDs breach default `ulimit -n` |
| Disk amplification | 1.29× | Sled 3.23× | Persistent per-bucket overhead |
| Concurrent (8 buckets) | 747 k op/s | fastest sharded | Parallel files, no shared lock |
| Proof read p99 | 7.7 µs | all < 10 µs | Not a differentiator |

The disqualifiers are concrete: **Sled needs 2.56 GiB for 1000 instances**, and
**RocksDB's 7000 FDs** breach a default 1024 limit and make pool sizing fragile.
SQLite also brings the non-performance wins the issue noted: one inspectable file
per bucket, ubiquitous tooling, and a battle-tested engine.

### Trade-off accepted

- **Write throughput ~4.5× below RocksDB sharded** (0.63 M vs 2.86 M op/s, 48-byte
  appends). Irrelevant: an MMR checkpoint writes a handful of positions.
- **Single writer per database** — a non-issue *because* we sharded: one provider
  writes one bucket file; different buckets write fully in parallel (the concurrent
  result confirms this). This is precisely the property that would have hurt a
  *shared*-SQLite design.
- **FFI to C** — SQLite is the most audited C library in existence; no read-latency
  penalty observed (2.7 µs warm p50).

### What would change this

- **Very high simultaneously-hot bucket counts (≫ pool cap, e.g. 10 k+ hot)**
  where even SQLite's per-instance RAM/FD growth strains the host: reconsider the
  **shared + RocksDB** combination, which had the best shared-DB profile (2.3 M
  op/s concurrent, 1.4 MiB / 7 FD, least-bad deletion). The crossover is a
  function of the LRU pool cap, not total buckets.
- A single bucket becoming write-hot at sustained millions of ops/s, or needing
  multi-writer concurrency within one bucket: revisit RocksDB for that tier.

---

## Summary

**Sharded (one DB per bucket) + SQLite in WAL mode.** Sharding wins the operations
tied to the bucket lifecycle — deletion, concurrent writes, fault isolation — and
the LRU pool bounds the memory that the shared model would otherwise win on.
Within the sharded model SQLite is the cheapest engine to open (39 µs) with the
lowest per-instance footprint.

The sharded-vs-shared decision was measured both ways (not assumed); the matrix
and crossover conditions are in [01](01-storage-provider-benchmark.md). If a
future operating point pushes simultaneously-hot bucket counts far past the LRU
pool cap, **shared + RocksDB** is the evidence-backed alternative.

Next: [03-configuration-guide.md](03-configuration-guide.md) ·
[04-migration-plan.md](04-migration-plan.md)
