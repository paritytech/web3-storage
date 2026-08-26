# Database Engine Evaluation

This directory holds the benchmark-backed evaluation of database engines for the
**Storage Provider**, the resulting recommendation, and the configuration and
migration guidance that follows from it.

The scope is deliberately one component. The chain side is **out of scope**: this
system runs on **Asset Hub**, so we neither operate the parachain node nor choose
its state-trie backend, and any recommendation there would be unactionable.

What is benchmarked: **Sled vs SQLite (WAL) vs redb vs RocksDB vs ParityDB**,
across **both** candidate architectures — one DB per bucket (sharded) and one DB
for all buckets (shared) — plus **LMDB, libmdbx and jammdb** as sharded-only
candidates, the mmap'd single-file B+trees built to be instantiated many times
over.

## Documents

| # | Document | Contents |
|---|----------|----------|
| — | [README.md](README.md) | This index + methodology + fairness caveats |
| 1 | [01-storage-provider-benchmark.md](01-storage-provider-benchmark.md) | Architecture × engine matrix across five measurement passes: sharded vs shared, eight engines, per-instance costs, the content store, and the SQLite `page_size` study |
| 2 | [02-recommendations.md](02-recommendations.md) | Recommendation + justification |
| 3 | [03-configuration-guide.md](03-configuration-guide.md) | SQLite/LRU-pool config, memory budget, bucket deletion |
| 4 | [04-migration-plan.md](04-migration-plan.md) | Single-RocksDB → chosen per-bucket architecture |
| 5 | [05-per-bucket-store-design.md](05-per-bucket-store-design.md) | Two stores per bucket (content + commitment): layout, durability barrier, crash-consistency invariant |

## The harness

All numbers in these documents come from a real, runnable harness committed at
[`benchmarks/db-bench/`](../../../benchmarks/db-bench). It exposes a common
`KvStore` trait over eight embedded engines — RocksDB, Sled, SQLite (WAL,
bundled), redb, ParityDB, LMDB (via `heed`), libmdbx and jammdb — and runs
workloads modelled on the Storage Provider's actual access patterns.

The suite runs **one process per engine**, merged afterwards by
[`scripts/merge-db-bench-results.py`](../../../scripts/merge-db-bench-results.py):
the whole matrix does not fit one process's scratch space, and isolating engines
means one dying cannot discard the others' results.

```bash
# Full run (writes JSON into results/ next to this README)
just db-bench
# or directly:
bash scripts/run-db-benchmarks.sh

# Fast smoke run (tiny sizes, for wiring checks)
just db-bench --quick
```

Raw results live in [`results/`](results/), **one file per measurement pass**:

| Pass | File | Adds |
|---|---|---|
| 1 | `storage-provider.json` | the original architecture × engine matrix (tmpfs) |
| 2 | `storage-provider-compaction-run.json` | redb + post-compaction disk (tmpfs; sled OOM-killed) |
| 3 | `content-store-run.json` | the `content_store` scenario (tmpfs) |
| 4 | `per-instance-run.json` | LMDB/libmdbx/jammdb + threads, address space, empty-bucket floor — **disk-backed**, all eight engines |
| — | `sqlite-page-size-run.json` | the SQLite `page_size` sweep + replicates |
| 5 | `final-run.json` | harness v2: retired scenarios removed, dedup-miss measured, SQLite durability mapping corrected |
| — | `dedup-experiment.json` | why the dedup lookup was slow: WAL ruled out, mmap worth 4.3×, cause found in the schema, fix measured |

Passes are **not** comparable to each other — same-host reruns moved absolute
figures by 25–48%, and passes 4–5 changed the storage medium — so every table
cites the single pass it comes from. `DB_BENCH_OUTPUT=<name>.json` records a new
pass without overwriting an existing one.

The crate is **its own workspace**, listed under `exclude` in the root
`Cargo.toml` and carrying its own `Cargo.lock`. The heavy engine dependencies
are therefore absent from the main workspace entirely — not just from its
builds and CI runs, but from its lockfile, so `cargo fetch` and `cargo vendor`
never download them either. Build it with the `just db-bench` recipe or:

```bash
cargo build --release --manifest-path benchmarks/db-bench/Cargo.toml
```

The trade-off is that root-level commands — `cargo check`/`clippy`/`test
--workspace`, `cargo fmt --all`, `cargo deny` — no longer reach this crate, so
CI does not lint it. That is the intent: it is a local research harness, not
shipped code, and it is validated by building and running it. When editing it,
point the tools at its manifest explicitly:

```bash
cargo clippy --manifest-path benchmarks/db-bench/Cargo.toml --all-targets
cargo +nightly fmt --manifest-path benchmarks/db-bench/Cargo.toml --all
```

## Methodology

- **Workloads model the two real stores**, as specified in
  [05-per-bucket-store-design.md](05-per-bucket-store-design.md):
  - *Commitment store* — 48-byte `MmrLeaf` values under dense position keys,
    durable per transaction: `mmr_append_small`, `proof_read`, `disk_small`,
    `open_close`, `empty_floor`, `multi_instance`.
  - *Content store* — 256 KiB chunks under **random content-hash keys**, unsynced
    batch ingest behind one flush barrier, plus the absent-key dedup lookup every
    upload performs before writing: `content_store`.
  - Architecture comparison — the same per-bucket workload run **sharded** (one
    DB file per bucket) and **shared** (one DB, keys `bucket_id || position`),
    with `concurrent_write` driving 8 threads against each.

  Passes 1–4 also ran `node_append_large` and `disk_large`: 256 KiB values under
  *sequential position keys with a per-batch fsync*. No such load exists in the
  design — chunks are hash-keyed with barrier durability, position-keyed values
  are 48 bytes — so they were retired in pass 5 once `content_store` covered the
  real large-value path. Their figures remain in the pass-1..4 files.
- **Reproducible.** A fixed RNG seed drives all data generation.
- **Latency** is reported as p50/p90/p99/max in microseconds (nearest-rank
  percentiles over per-operation samples). **Throughput** is ops/s and MiB/s
  over the wall-clock window. **RSS** (`/proc/self/statm`), **virtual address
  space** (same file — the metric that bounds mmap engines reserving a fixed map
  per instance), **thread count** (`/proc/self/task`) and **open file
  descriptors** (`/proc/self/fd`) are sampled with all instances live.
- **On-disk size is measured in allocated blocks**, not apparent file length.
  Engines that map a large sparse file (LMDB, redb, libmdbx) would otherwise
  report their declared ceiling as their footprint — redb's apparent size
  overstates its allocated blocks by 32× on an empty store.
- **Durability.** A `sync` batch means the strongest cheap durability each
  engine offers; the mappings are not identical and the differences are stated
  rather than smoothed over:

  | Engine | `sync` batch | `flush()` |
  |---|---|---|
  | SQLite | commit at `synchronous = FULL` (fsyncs the WAL) | `wal_checkpoint(TRUNCATE)` |
  | RocksDB | `WriteOptions::sync` | `flush()` |
  | redb | `Durability::Immediate` | empty immediate commit |
  | Sled | `flush()` after the batch | `flush()` |
  | LMDB (`heed`) | commit then `force_sync()` (env opened `NO_SYNC \| NO_META_SYNC`) | `force_sync()` |
  | libmdbx | commit then `sync(true)` (opened `SafeNoSync`) | `sync(true)` |
  | jammdb | **no knob** — every commit is durable, so `sync` cannot be honoured downward | no-op |
  | ParityDB | **no per-commit fsync** — async log + worker | — |

  Two asymmetries to carry into any reading: **jammdb's unsynced numbers are
  pessimistic** (it always pays full durability) and **ParityDB's write numbers
  sit at a weaker durability point** than everyone else's.

  Passes 1–4 mapped a SQLite `sync` batch to a full `wal_checkpoint(TRUNCATE)` —
  strictly more work than production does, since it fsyncs *and* folds the WAL
  back into the main file on every batch. Pass 5 corrected it to
  `synchronous = FULL`, which is what the commitment store actually runs; SQLite's
  durable-append figures rise ~17× as a result.

## Fairness and validity caveats (read before trusting absolute numbers)

> [!IMPORTANT]
> The deliverable is the **relative ranking** between engines, not the absolute
> figures. The figures were produced in the development container below; re-run
> the harness on representative target hardware before committing budget to an
> engine.

> [!IMPORTANT]
> **Passes 4–5 changed the storage medium.** Passes 1–3 used the tmpfs scratch
> described below; passes 4–5 used a disk-backed filesystem, because tmpfs both
> hit `ENOSPC` at 3.9 GiB and consumed the RAM that OOM-killed sled in passes 2–3
> (sled completes from pass 4 onward). The write-related caveats below apply in
> full to passes 1–3 and not to passes 4–5.
>
> Pass 4 additionally reported that this reversed the sharded-vs-shared
> concurrency result. **It did not** — that was a harness bug in which the two
> architectures paid different durability, [retracted in
> pass 5](01-storage-provider-benchmark.md#retracted-the-concurrency-inversion-was-a-harness-bug).

**Benchmark host (recorded with the run):**

| | |
|---|---|
| Kernel | `6.12.76-linuxkit` (containerized) |
| CPUs | 10 |
| RAM | 7.65 GiB |
| Scratch FS | passes 1–3: **tmpfs (RAM-backed)** at `/tmp`; pass 4: disk-backed at `/var/tmp` |
| `ulimit -n` | 1048576 |
| Page size | 4096 B |

**What this means per metric:**

- **Write / commit / fsync latency and throughput — OPTIMISTIC.** The scratch
  filesystem is tmpfs, so `fsync` has no real disk barrier. Absolute write
  numbers (e.g. millions of synced ops/s) are far higher than any real SSD will
  deliver. *Relative* ordering between engines still holds (all share the same
  FS), but do not quote the absolute write throughput as production capacity.
Passes 1–3 warned that tmpfs makes the shared-SQLite single-writer penalty
  *look milder than it is*, and treated the measured sharded-vs-shared gap as a
  floor. Moving to disk in pass 4 appeared to invert the result outright — but
  that was [a harness bug](01-storage-provider-benchmark.md#retracted-the-concurrency-inversion-was-a-harness-bug),
  not a medium effect. Measured fairly on disk in pass 5, sharded leads by 1.60×,
  inside the 1.04–1.75× band passes 1–3 reported. The tmpfs caveat on *absolute*
  write throughput stands; the sharded-vs-shared *ranking* turned out to be
  medium-independent.
- **Read latency — REPRESENTATIVE.** Point-lookup latency reflects engine code
  paths, FFI overhead, and index structure, which tmpfs does not distort.
- **On-disk size / space amplification — VALID.** File sizes are real bytes.
- **File-descriptor counts — VALID.** Independent of storage medium.
- **RSS — VALID, with the usual page-granular noise** on small deltas. The
  large, monotonic deltas (e.g. Sled's per-instance growth) are real signal.

No bare-metal SSD path was available in this environment (the repo mount is
virtiofs, `/` is overlay2 — both virtualized), so tmpfs was chosen as the
cleanest *consistent* surface for cross-engine comparison.
