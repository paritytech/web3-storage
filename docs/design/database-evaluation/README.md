# Database Engine Evaluation

This directory holds the benchmark-backed evaluation of database engines for the
two storage components, the recommendation for each, and the configuration and
migration guidance that follows from it.

The Storage Provider and Blockchain Node have **different database workload
profiles**. This evaluation treats them separately and benchmarks the candidates
the issue calls out:

- **Storage Provider** (per-bucket DBs): **Sled vs SQLite (WAL) vs RocksDB**, across **both** the sharded and shared architectures
- **Blockchain Node** (state trie): **RocksDB vs ParityDB**

## Documents

| # | Document | Contents |
|---|----------|----------|
| — | [README.md](README.md) | This index + methodology + fairness caveats |
| 1 | [01-storage-provider-benchmark.md](01-storage-provider-benchmark.md) | Architecture × engine matrix (sharded vs shared; Sled/SQLite/RocksDB/ParityDB) |
| 2 | [02-blockchain-provider-benchmark.md](02-blockchain-provider-benchmark.md) | Measured RocksDB vs ParityDB for the state trie |
| 3 | [03-recommendations.md](03-recommendations.md) | Recommendation + justification per component |
| 4 | [04-configuration-guide.md](04-configuration-guide.md) | Engine config, memory limits, compaction tuning, OS/cgroup isolation |
| 5 | [05-migration-plan.md](05-migration-plan.md) | Single-RocksDB → chosen per-bucket architecture |

## The harness

All numbers in these documents come from a real, runnable harness committed at
[`benchmarks/db-bench/`](../../../benchmarks/db-bench). It exposes a common
`KvStore` trait over RocksDB, Sled, SQLite (WAL, bundled), and ParityDB, and
runs workloads modelled on the actual access patterns of each component.

```bash
# Full run (writes JSON into results/ next to this README)
just db-bench
# or directly:
bash scripts/run-db-benchmarks.sh

# Fast smoke run (tiny sizes, for wiring checks)
just db-bench --quick
```

Raw results live in [`results/`](results/): `storage-provider.json` and
`blockchain-provider.json`. Every table below is derived from those files;
re-running the harness regenerates them.

The crate is intentionally **excluded from `default-members`** in the workspace
`Cargo.toml`, so normal `cargo build` / `cargo test` / CI do **not** pull in the
four heavy DB dependencies. It builds only via `cargo build -p db-bench` or the
`just db-bench` recipe.

## Methodology

- **Workloads model the real components.** Storage Provider scenarios cover
  *both* architectures Issue #100 is deciding between: **sharded** (one small DB
  per bucket, keyed by MMR leaf position, 48-byte `MmrLeaf` values plus a 256 KiB
  chunk-node variant) and **shared** (a single DB for all buckets, keyed by
  `bucket_id || position`). A `concurrent_write` scenario writes many buckets
  from 8 threads under each architecture — the decisive test, since a shared DB
  serializes SQLite's single writer while sharded files do not. Blockchain
  Provider scenarios use random 32-byte keys committed in per-block batches, the
  Substrate state-trie pattern.
- **Reproducible.** A fixed RNG seed drives all data generation.
- **Latency** is reported as p50/p90/p99/max in microseconds (nearest-rank
  percentiles over per-operation samples). **Throughput** is ops/s and MiB/s
  over the wall-clock window. **RSS** (`/proc/self/statm`) and **open file
  descriptors** (`/proc/self/fd`) are sampled with all instances live.
- **Durability.** Write scenarios commit in atomic batches with the engine's
  strongest cheap durability flag set (RocksDB `WriteOptions::sync`, Sled
  `flush()`, SQLite WAL checkpoint). ParityDB commits through an asynchronous
  log + worker and exposes no per-commit fsync, so its write numbers reflect a
  weaker durability point than the others — this asymmetry is flagged wherever
  it matters.

## Fairness and validity caveats (read before trusting absolute numbers)

> [!IMPORTANT]
> The deliverable is the **relative ranking** between engines, not the absolute
> figures. The figures were produced in the development container below; re-run
> the harness on representative target hardware before committing budget to an
> engine.

**Benchmark host (recorded with the run):**

| | |
|---|---|
| Kernel | `6.12.76-linuxkit` (containerized) |
| CPUs | 10 |
| RAM | 7.65 GiB |
| Scratch FS | **tmpfs (RAM-backed)** at `/tmp` |
| `ulimit -n` | 1048576 |
| Page size | 4096 B |

**What this means per metric:**

- **Write / commit / fsync latency and throughput — OPTIMISTIC.** The scratch
  filesystem is tmpfs, so `fsync` has no real disk barrier. Absolute write
  numbers (e.g. millions of synced ops/s) are far higher than any real SSD will
  deliver. *Relative* ordering between engines still holds (all share the same
  FS), but do not quote the absolute write throughput as production capacity.
  **One asymmetry to note:** tmpfs makes the *shared-SQLite single-writer penalty
  look milder than it is*, because the write lock is held only for a near-instant
  `fsync`. On real SSD that serialization would be markedly worse — so the
  measured sharded-vs-shared SQLite gap is a floor.
- **Read latency — REPRESENTATIVE.** Point-lookup latency reflects engine code
  paths, FFI overhead, and index structure, which tmpfs does not distort.
- **On-disk size / space amplification — VALID.** File sizes are real bytes.
- **File-descriptor counts — VALID.** Independent of storage medium.
- **RSS — VALID, with the usual page-granular noise** on small deltas. The
  large, monotonic deltas (e.g. Sled's per-instance growth) are real signal.

No bare-metal SSD path was available in this environment (the repo mount is
virtiofs, `/` is overlay2 — both virtualized), so tmpfs was chosen as the
cleanest *consistent* surface for cross-engine comparison.
