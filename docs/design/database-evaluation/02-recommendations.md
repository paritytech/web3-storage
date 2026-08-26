# Recommendations

Backed by the measured results in [01](01-storage-provider-benchmark.md). The
recommendation states the choice, the justification from the data, the trade-off
accepted, and the conditions that would change it.

Figures are labelled with the measurement pass they come from, because passes are
not comparable: **passes 1–3** ran on tmpfs, **passes 4–5** on disk-backed
storage, and pass 5 corrected a durability-mapping bug that had understated every
SQLite write figure. Where a number decides something, both are given.

Scope: the **Storage Provider** only. The chain runs on Asset Hub, so its
database backend is not ours to choose.

---

## Storage Provider → **Sharded (one DB per bucket) + SQLite (WAL)**

This is a two-part decision: first the *architecture* (sharded vs shared),
then the *engine*. Both are backed by the architecture × engine matrix
in [01](01-storage-provider-benchmark.md).

### Decision

1. **Architecture: sharded** — per-bucket database files, behind an LRU
   connection pool.
2. **Layout: two stores per bucket** — a *content store* (chunks + chunk-tree
   nodes, hash-keyed) and a *commitment store* (MMR leaves + interior nodes +
   bucket state, position-keyed), per
   [05-per-bucket-store-design.md](05-per-bucket-store-design.md). The global
   node pool is dropped (client-side encryption forecloses cross-bucket dedup).
3. **Engine: SQLite (WAL mode) for both stores**, with different durability
   settings: the commitment store runs fully synchronous (it holds the
   slashable state); the content store runs relaxed with one flush barrier
   before each commitment (its writes are idempotent and hash-verified).
4. **The content store keeps its hash index in a separate B-tree** from the chunk
   payloads, and enables `mmap_size`. This is not a micro-optimisation: measured,
   it is worth **50× on the dedup lookup and 4.8× on chunk reads**, and it is what
   makes SQLite competitive with LMDB on the content store at all. See the
   [dedup experiment](01-storage-provider-benchmark.md#the-dedup-experiment-three-hypotheses-one-cause-one-fix).

Note the pool arithmetic: two stores per bucket doubles per-bucket FDs and
instances, so [report 01](01-storage-provider-benchmark.md)'s `multi_instance`
figures apply ×2 at a given pool size — SQLite's 6 FDs and ~60–140 MiB per
1000 *hot* buckets remains comfortably the smallest footprint of any viable
pair.

### Why sharded over shared

The two architectures genuinely trade off, and we measured both:

| What it wins | Architecture | Measured |
|--------------|--------------|----------|
| Resource footprint | **Shared** | one flat instance, 1–34 MiB / 2–7 FDs for 1000 buckets vs sharded's per-instance growth |
| Bucket deletion | **Sharded** | `unlink` ~0.1 ms, 100% reclaim vs shared in-place delete 19–141 ms with **no reclaim** (ParityDB *grew 8×*) |
| Concurrent writes | **Sharded** | 1.04–1.75× faster on tmpfs (passes 1–3); **1.60× on disk, winning 6 of 7 paired runs** (pass 5) |
| Fault isolation | **Sharded** | one corrupt file = one bucket, not the whole store |

Pass 4 briefly reported the concurrency row inverted — shared 24× faster — and
that finding is [withdrawn](01-storage-provider-benchmark.md#retracted-the-concurrency-inversion-was-a-harness-bug):
the two architectures had not been paying the same durability. Re-measured
fairly, sharded leads, and the ranking holds on both media.

Sharded wins on the operations tied to the **bucket lifecycle** — creation,
expiry, deletion — which the issue explicitly flags as a tombstone-spike
bottleneck. The shared model's headline advantage is total memory, and that is
**neutralized by the LRU pool**: in the sharded model only *hot* buckets are
open, so RAM/FDs are bounded by the pool cap, not the bucket count, while cold
buckets are just cheap files.

**Two further points favour shared, and the table above overstates the case
against it.** Both are worth stating plainly rather than leaving for someone to
rediscover:

- **A shared database gives the crash-ordering guarantee for free.** One database
  is one WAL, and WAL recovery is prefix-consistent, so a recovered commitment
  necessarily implies the content writes that preceded it survived. The
  content-before-commitment barrier that
  [05](05-per-bucket-store-design.md#the-crash-consistency-invariant-the-one-cost-of-two-databases)
  makes an application obligation — one choke-point function, a crash-injection
  test — exists *only because* we split into separate files. This is the same
  mechanism as 05's single-file fallback, one level up.
- **Shared deletion is less bad than the per-key numbers suggest.** With keys
  laid out `bucket_id || …` a bucket is a contiguous key range, and RocksDB's
  `DeleteRange` / `DeleteFilesInRange` drop whole SST files for interior ranges
  rather than writing a tombstone per key. Combined with the finding that
  compaction *does* reclaim ([Deletion, revisited](01-storage-provider-benchmark.md#deletion-revisited-compaction-reclaims-what-tombstones-defer)),
  shared deletion is "milliseconds of range-delete plus deferred reclaim", not
  the catastrophe the per-key figures imply.

**What still decides it for sharded**, weighing those in:

- **Fault isolation, priced by slashing.** One corrupt page in a shared store puts
  *every* bucket's commitments in question simultaneously — provider-fatal, every
  agreement at risk. Sharded, the same corruption costs one bucket, one
  agreement; and content is hash-verified and re-fetchable from replicas, so even
  that is often recoverable. In a system where data loss is financially
  catastrophic by design, blast radius is a first-order property, not a nicety.
- **Compaction blast radius.** Compaction debt is pooled in a shared store: one
  churn-heavy bucket's tombstones throttle writes and fatten p99s for every other
  bucket while they settle. Sharding is noisy-neighbour isolation for background
  I/O — the performance twin of the fault-isolation argument. And `unlink` is
  effectively **O(1) compaction**: in the sharded model the unit of deletion *is*
  the unit of reclamation, so bucket expiry mints no debt at all, whereas a
  shared store's deletion unit (a key range) is always smaller than its
  reclamation unit (an SST or page).
- **The bucket as an operational unit** — a file you can copy, back up, migrate,
  hand to a replica, or open with the `sqlite3` CLI during an incident, rather
  than a key range inside a multi-TB store.

Hence sharded: not on throughput or footprint, but because this protocol converts
rare storage failures into large financial events, and sharding is what keeps
those failures small and per-agreement.

### Why SQLite within the sharded model

The sharded cost drivers are reopen latency, per-instance memory, and FD
pressure — not raw throughput. SQLite wins or ties every one:

| Driver | SQLite | vs. alternatives | Why it matters |
|--------|--------|------------------|----------------|
| Reopen latency | **39 µs** p50 (p1) / **87 µs** (p4) | 11× faster than Sled, 27× faster than RocksDB (p1); RocksDB ~5.0 ms on disk (p4) | Paid on every LRU reload |
| RSS @ 1000 instances | **31.6 MiB** (p1) / **71.6 KiB per instance** (p4) | RocksDB 175 KiB/inst; **Sled 2.6 MiB/inst** (p4) | Determines how many buckets fit in RAM |
| FDs / instance | 3 | Sled 1, RocksDB 7 | RocksDB's 7000 FDs breach default `ulimit -n` |
| Threads / instance | **0** (p4) | libmdbx **1.00**; RocksDB ~0 (fixed pool) | 1000 buckets must not mean 1000 threads |
| Disk amplification | 1.29× | Sled 3.14×, jammdb 4.52×, libmdbx 3.00× (p4) | Persistent per-bucket overhead |
| Empty-bucket floor | **8 KiB** (p4) | RocksDB 44 KiB, jammdb 128 KiB, libmdbx 256 KiB | Charged per provisioned bucket |
| Concurrent (8 buckets) | 747 k op/s (p1) / **109 k op/s** (p5) | fastest sharded on tmpfs; 1.60× over shared on disk | Parallel files, no shared lock |
| Proof read p99 | 7.7 µs (p1) / 12.0 µs (p4) | all < 25 µs | Not a differentiator |

The disqualifiers are concrete: **Sled needs 2.56 GiB for 1000 instances**, and
**RocksDB's 7000 FDs** breach a default 1024 limit and make pool sizing fragile.
SQLite also brings the non-performance wins the issue noted: one inspectable file
per bucket, ubiquitous tooling, and a battle-tested engine.

### Per-store engine choice

The two stores have opposite workloads, so the choice was re-examined per
store against passes 2–5
([redb section](01-storage-provider-benchmark.md#redb--a-fifth-candidate-second-measurement-pass),
[content-store section](01-storage-provider-benchmark.md#the-content-store-measured-third-pass),
[per-instance and LMDB](01-storage-provider-benchmark.md#reading-the-fourth-pass),
[dedup experiment](01-storage-provider-benchmark.md#the-dedup-experiment-three-hypotheses-one-cause-one-fix)):

| | Commitment store (48 B, position keys, fully durable) | Content store (256 KiB, hash keys, barrier durability) |
|---|---|---|
| **SQLite** | **best space** (1.29×→1.13× compacted), sequential-insert packing, zero threads, 8 KiB floor, multi-table transactions for leaf+node+state atomicity | 1.00× space; **34.6 µs reads and 3.9 µs dedup with a split index + mmap** (711 µs / 746 µs at the harness defaults) |
| **LMDB** | best on nearly every driver: 45 µs reopen, 0.46/0.71 µs proof reads, 1.23× space, zero threads | **fastest reads by far (19 µs)** at 1.02× space and 536 MiB/s ingest |
| redb | best per-instance profile, but 3.01× space that *compaction worsens* to 4.65× | fast reads (26 µs), but **2.00× space** — doubles the capacity bill; 1.7 s flush barrier |
| RocksDB | 1.10× space, but ~1.1 ms (p1) to ~5.0 ms (p4) reopens and 7 FDs/instance ruin the LRU pool | 1.00× space, good reads (159 µs); same reopen/FD/compaction liabilities, now ×2 per bucket |
| jammdb / libmdbx | disqualified: 4.52× space; 1 thread per instance and 1.2 GiB unreclaimed after deleting every key | — |

**SQLite for both.** For the commitment store it wins or ties every driver that
matters. For the content store it ties the best space figure — the cost a
capacity-priced provider actually pays — and its one measured weakness,
chunk-read latency, is sub-millisecond and partly tuned away: `page_size = 32768`
buys a measured **1.58×** (452 → 286 µs warm p50, seven runs, disjoint ranges).
Running one engine everywhere also keeps the operational surface — WAL discipline,
backup, `integrity_check`, tooling — singular.

The honest caveat is that **LMDB is faster on the content store than tuning gets
SQLite**, by roughly 15× on chunk reads even after the page-size fix. The next
subsection is why that does not change the recommendation.

### Is the SQL layer wasted overhead? (measured)

The recurring instinct — "we only need a KV store, SQL is dead weight" — was
tested directly by adding **redb**, which is precisely SQLite's index family
(a B-tree) without the SQL layer. The measured answer:

- The SQL layer's cost is real but small and off the critical path: ~2–4 µs
  per point read on 48 B values (redb 0.79 µs vs SQLite 3.17 µs p50) with
  prepared statements — parsing is amortised away; what remains is VDBE
  dispatch and FFI. At MMR-proof scale (a handful of reads per challenge)
  this is noise.
- The things that *are* on the critical path — disk amplification, reopen
  latency, crash-recovery maturity, multi-table atomic transactions (leaves +
  MMR nodes + state in one commit) — SQLite wins or ties every time, and the
  SQL-free alternative loses on the binding one (space: 3.01×/4.65× small
  values, 2.06× large).
- Conclusion: the SQL layer is an aesthetic cost, not a performance one, on
  this workload. What SQLite is *actually* paying for its C heritage and SQL
  machinery is repaid by `balanceQuick` packing, WAL maturity, and
  transactions that map exactly onto the commitment store's atomicity needs.

### Why not LMDB (yet)

LMDB leads SQLite on every *performance* driver in the sharded model — 45 vs
87 µs reopen, 0.46 vs 4.00 µs proof reads, 19 vs 656 µs chunk reads, 1.02× vs
1.00× space, zero background threads (pass 4). It is the first candidate to lead
on the merits rather than on one axis, and the recommendation stays with SQLite
anyway. The reasons are not performance ones.

**Reliability is not a tie.** SQLite is tested to a standard essentially nothing
else in this class matches: [TH3](https://sqlite.org/th3.html) achieves **100%
MC/DC branch coverage**, the metric DO-178B avionics certification requires,
applied to every release since 2009 — and SQLite ships in the Airbus A350's
flight software. LMDB's case is a long track record (OpenLDAP, Monero,
Meilisearch) and a small, simple design, which is a real argument, but it is
"battle-tested" rather than "systematically verified", and upstream is
effectively in maintenance mode.

**Three LMDB failure modes map directly onto this workload:**

1. **Long-lived read transactions block page reuse.** Per LMDB's own caveats, no
   space is reclaimed while a read transaction is open, so the file grows
   append-only until it closes. The code that holds a read open is exactly the
   code that streams a large download or builds a challenge proof.
2. **No online compaction.** Measured: 3.3 MiB retained after deleting every key,
   with no reclamation API. Harmless while buckets die by `unlink`; decisive if a
   bucket is ever cleared in place.
3. **`MDB_MAP_FULL` is a cliff**, and `map_size × pool_cap` is an address-space
   budget no other engine imposes — measured at exactly 1 GiB per instance,
   1001.5 GiB across a 1000-bucket pool. For buckets holding chunked media the
   per-bucket ceiling has to be large, which makes the pool cap the binding
   constraint.

**The finding that looked like it would change this, and did not.** Pass 5
measured SQLite answering the dedup check — the absent-key lookup every upload
performs — in **746 µs against LMDB's 0.50 µs**, on the upload hot path. A
[four-part experiment](01-storage-provider-benchmark.md#the-dedup-experiment-three-hypotheses-one-cause-one-fix)
found the cause was neither the engine nor the WAL but the **schema**: storing
256 KiB payloads in a `WITHOUT ROWID` table puts them inside the B-tree a key
search descends. Giving the hash its own index over a rowid table, plus enabling
`mmap_size`, closes it:

| | As measured | Tuned | LMDB |
|---|---:|---:|---:|
| Dedup miss p50 | 746 µs | **3.92 µs** | 0.50 µs |
| Chunk read warm p50 | 711 µs | **34.58 µs** | 19 µs |

Both of SQLite's headline weaknesses were artifacts of how the harness configured
it. Tuned, it is within **1.8×** of LMDB on chunk serving and **7.8×** on dedup,
at identical disk amplification — and the schema change makes ingest 17% faster
rather than costing anything. This is now a much weaker case for changing engines
than it was before the experiment, not a stronger one.

**And its write numbers are a best case.** The harness opens LMDB
`NO_SYNC | NO_META_SYNC`. Per LMDB's documentation `metasync=False` alone
preserves integrity (losing at most the last transaction), but `sync=False`
*"can corrupt the database or lose the last transactions"*, safe only because
`writemap=False` **and** the filesystem preserves write order. A production
deployment would want the stronger setting, and would be slower than measured
here. SQLite's WAL at `synchronous = FULL` carries no such conditional.

**What the gap is actually worth.** Chunk-read latency is the only place LMDB's
advantage is visible end-to-end. Reassembling 1 GiB (4096 × 256 KiB chunks) costs
1.17 s of database reads with tuned SQLite versus 0.08 s with LMDB — 13.6% versus
0.9% of an 8.6 s transfer on a 1 Gbps link. Real, but two cheaper mitigations come
first: reassembly parallelises across WAL readers (that figure is
single-threaded), and the O(n)→O(log n) descent fix in
[05](05-per-bucket-store-design.md) removes a full-tree DFS that currently dwarfs
it.

Providers **lock stake and are slashed for data loss**, so a corruption bug costs
money rather than uptime. Against that, a 15× read gap on a path that is already
sub-millisecond and further mitigable does not justify trading the most
rigorously tested storage engine available for one whose reclamation behaviour
degrades under precisely the read pattern this system performs.

**What would change it:** the content store only, on SSD-validated numbers, if
chunk-serving latency is still binding after parallel reassembly and the descent
fix — with the long-lived-read-transaction hazard addressed by a hard cap on read
transaction lifetime.

### Trade-off accepted

- **Write throughput below RocksDB sharded** (pass 1: 0.63 M vs 2.86 M op/s on
  48-byte appends). Irrelevant: an MMR checkpoint writes a handful of positions,
  and pass 5 puts SQLite's durable-append rate at 96.8 k op/s once a `sync` batch
  means an fsync rather than a full WAL checkpoint.
- **Single writer per database** — bounded *because* we sharded: one provider
  writes one bucket file, and different buckets write in parallel. Pass 5 measures
  sharded at 1.60× shared under equal durability, so this is a property the
  architecture exploits rather than a cost it pays.
- **Chunk reads are the slowest of any candidate**, even tuned: 286 µs versus
  LMDB's 19 µs. Accepted deliberately — see [Why not LMDB (yet)](#why-not-lmdb-yet).
- **FFI to C** — SQLite is the most audited C library in existence; no read-latency
  penalty observed.

### What would change this

- **Very high simultaneously-hot bucket counts (≫ pool cap, e.g. 10 k+ hot)**
  where even SQLite's per-instance RAM/FD growth strains the host: reconsider the
  **shared + RocksDB** combination, which had the best shared-DB profile (2.3 M
  op/s concurrent, 1.4 MiB / 7 FD, least-bad deletion). The crossover is a
  function of the LRU pool cap, not total buckets.
- A single bucket becoming write-hot at sustained millions of ops/s, or needing
  multi-writer concurrency within one bucket: revisit RocksDB for that tier.
- **The content-before-commitment barrier proving unenforceable** across the three
  upload paths: fall back to [one file with two tables per bucket](05-per-bucket-store-design.md#the-single-file-fallback),
  which restores the ordering guarantee at the engine level. That is a
  bucket-local, reversible change — unlike moving to a shared database, which
  also restores it but forfeits fault isolation.
- **Chunk-serving latency becoming the binding constraint** (large files,
  latency-sensitive reads): move the *content store only* to **LMDB** (19 µs warm
  reads, 1.02× space) or **RocksDB** (159 µs, 1.00× space), keeping SQLite for the
  commitment store. Try `page_size = 32768` (1.58×), parallel reassembly, and the
  O(log n) descent fix first — all three are cheaper than an engine change — and
  validate on SSD, since read rankings are the least medium-robust numbers here.

---

## Summary

**Sharded, two SQLite stores per bucket, WAL mode.** Sharding wins the operations
tied to the bucket lifecycle — deletion, concurrent writes (1.60×, pass 5), fault
isolation — and the LRU pool bounds the memory that the shared model would
otherwise win on. Within the sharded model SQLite is among the cheapest engines to
open, spawns no threads, has the smallest empty-bucket floor, and ties the best
disk amplification. **LMDB is faster on every read path**; SQLite is kept for
testing rigour and operational safety — see [Why not LMDB (yet)](#why-not-lmdb-yet).

The sharded-vs-shared decision was measured both ways (not assumed); the matrix
and crossover conditions are in [01](01-storage-provider-benchmark.md). If a
future operating point pushes simultaneously-hot bucket counts far past the LRU
pool cap, **shared + RocksDB** is the evidence-backed alternative.

Next: [03-configuration-guide.md](03-configuration-guide.md) ·
[04-migration-plan.md](04-migration-plan.md)
