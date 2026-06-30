# Recommendations

Backed by the measured results in [01](01-storage-provider-benchmark.md) and
[02](02-blockchain-provider-benchmark.md). Each recommendation states the choice,
the justification from the data, the trade-off accepted, and the conditions that
would change it.

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

## Blockchain Node (state trie) → **ParityDB**

### Decision

Run the parachain node with the **ParityDB** backend (`--database paritydb`),
paired with the mandatory memory-isolation mitigation below.

### Justification (from the data)

The state-trie workload is dominated by random 32-byte-key point reads, which is
precisely what ParityDB's hash-indexed tables are built for:

| Driver | ParityDB | RocksDB | Margin |
|--------|---------:|--------:|--------|
| Cold state read p50 | **1.0 µs** | 14.5 µs | **14× faster** |
| Warm state read p50 | **0.9 µs** | 3.3 µs | 3.7× faster |
| Read p99 (warm) | **4.3 µs** | 27.1 µs | tighter tail |
| Block import p50 | **80.5 µs** | 131.0 µs | ~40% faster |
| On-disk after import | **14.6 MiB** | 20.4 MiB | ~28% smaller |

Faster state reads improve block execution and RPC state queries directly, and
this matches why upstream Substrate offers ParityDB as the optimized state
backend. Lower write amplification (append-oriented tables vs. LSM compaction)
also addresses the issue's **SSD compaction-wear** concern for RocksDB.

### Trade-off accepted

- **Higher, page-cache-driven memory.** ParityDB peaked at **396 MiB vs RocksDB's
  94 MiB** under sustained load because it relies on the OS page cache rather than
  an explicit bounded block cache. This is the issue's "page-cache starvation"
  risk and makes the **cgroup isolation mitigation (Step 2) mandatory**, not
  optional — see [04-configuration-guide.md](04-configuration-guide.md).
- **Moderate maturity** vs. RocksDB's very-high maturity. Mitigated by ParityDB
  being Parity-maintained and the default state backend on production Polkadot
  infrastructure.
- **No synchronous space reclamation on pruning** (it grew on bare delete) — but
  RocksDB shares this; both need scheduled compaction / background reclamation.

### What would change this

If the node is deployed in a tightly memory-constrained environment where the
cgroup cap cannot be set high enough for ParityDB's page-cache working set, or if
an operator requires the maximal-maturity option, RocksDB remains a fully
supported fallback (`--database rocksdb`) with the Step 3 compaction tuning
applied. The synthetic ranking should be **confirmed with the node-level A/B run**
in [02](02-blockchain-provider-benchmark.md) before final commitment.

---

## Summary

| Component | Choice | One-line reason |
|-----------|--------|-----------------|
| Storage Provider | **Sharded + SQLite (WAL)** | Sharded wins bucket-delete/concurrency/isolation; LRU pool bounds its memory; SQLite is the cheapest sharded engine to open with the lowest footprint |
| Blockchain Node (state trie) | **ParityDB** | 14× faster cold state reads + smaller on-disk; bound its memory with cgroups |

The sharded-vs-shared decision was measured both ways (not assumed); the matrix
and crossover conditions are in [01](01-storage-provider-benchmark.md). If a
future operating point pushes simultaneously-hot bucket counts far past the LRU
pool cap, **shared + RocksDB** is the evidence-backed alternative.

Next: [04-configuration-guide.md](04-configuration-guide.md) ·
[05-migration-plan.md](05-migration-plan.md)
