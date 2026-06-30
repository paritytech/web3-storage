# Blockchain Provider Benchmark — RocksDB vs ParityDB

**Component:** Blockchain Provider, Substrate state-trie backend.
**Source data:** [`results/blockchain-provider.json`](results/blockchain-provider.json) (seed `13371337`, scale `1.0`).
**Read the [methodology and caveats](README.md#methodology) first.**

## Scope and honesty about the model

The Blockchain Provider stores the state trie through Substrate's `sc-client-db`,
which supports both RocksDB and ParityDB backends. Benchmarking the *true*
end-to-end path requires a running node, so this harness is a **synthetic
microbenchmark on the raw key/value engines** exercising the state-trie access
pattern: random 32-byte keys, batched commits per block, random point reads, and
pruning. It isolates engine behaviour under that pattern; it is **not** a full
node measurement.

> [!NOTE]
> **Recommended follow-up validation (run on a node, not in CI):** spawn the
> parachain twice — once with `--database rocksdb`, once with `--database
> paritydb` — drive identical load, and compare block-import time, state-read
> latency, on-disk size, and steady-state RSS. The synthetic results below
> predict the ranking; the node run confirms magnitude. See
> [04-configuration-guide.md](04-configuration-guide.md) for the exact flags.

One durability asymmetry to keep in mind: RocksDB commits here fsync per block;
ParityDB has no per-commit fsync and finalizes through its async log + worker, so
its write latency is measured at a weaker durability point.

## Headline results

| Criterion | RocksDB | ParityDB | Winner |
|-----------|--------:|---------:|:------:|
| Block import, per-block commit p50 (µs) | 131.0 | **80.5** | ParityDB |
| Block import throughput (op/s) | 1,236,329 | **1,767,798** | ParityDB |
| State read, **cold** p50 / p99 (µs) | 14.5 / 38.0 | **1.0 / 3.9** | ParityDB |
| State read, warm p50 / p99 (µs) | 3.3 / 27.1 | **0.9 / 4.3** | ParityDB |
| On-disk size after import | 20.4 MiB | **14.6 MiB** | ParityDB |
| Sustained throughput (op/s) | 937,795 | **1,036,422** | ~tie |
| Sustained peak RSS | **93.9 MiB** | 396 MiB | RocksDB |
| Pruning — space reclaimed synchronously | none (grew) | none (grew) | neither |

## Per-scenario detail

### 1. Block import (batched durable commits)

1000 blocks × 200 state writes each (64-byte values), one durable batch per block.

| Engine | per-block p50 (µs) | per-block p99 (µs) | throughput (op/s) | on-disk |
|--------|-------------------:|-------------------:|------------------:|--------:|
| ParityDB | **80.5** | **116.5** | **1,767,798** | **14.6 MiB** |
| RocksDB | 131.0 | 243.2 | 1,236,329 | 20.4 MiB |

ParityDB imports ~40% faster and lands ~28% smaller on disk. (RocksDB's
durability point is stronger here — fsync per block — so part of the gap is the
durability asymmetry; the node-level follow-up resolves this.)

### 2. State read latency (the deciding metric)

1 M keys populated, DB reopened for a cold cache, then 100 k random point reads.

| Engine | cold p50 (µs) | cold p99 (µs) | warm p50 (µs) | warm p99 (µs) |
|--------|--------------:|--------------:|--------------:|--------------:|
| ParityDB | **1.0** | **3.9** | **0.9** | **4.3** |
| RocksDB | 14.5 | 38.0 | 3.3 | 27.1 |

This is where ParityDB's design pays off and matches the issue's analysis.
ParityDB's hash-indexed value tables are built for exactly this workload —
**32-byte-key point lookups** — and deliver **~14× faster cold reads** and ~3.7×
faster warm reads with a much tighter tail. State-trie traversal is overwhelmingly
random point reads, so this directly improves block execution and RPC state
queries. RocksDB's cold read suffers from LSM read amplification (checking
multiple SST levels + bloom filters) before the block cache warms.

### 3. Pruning (deleting historical state)

500 k keys, delete the oldest 250 k, reopen, measure disk reclaimed.

| Engine | delete throughput (op/s) | disk before | disk after | reclaimed |
|--------|-------------------------:|------------:|-----------:|----------:|
| ParityDB | 1,185,510 | 40.2 MiB | 76.5 MiB | **−90%** (grew) |
| RocksDB | 584,790 | 50.9 MiB | 60.7 MiB | **−19%** (grew) |

**Neither engine reclaims space synchronously on delete**, and this is the
honest, important finding: a bare delete + reopen leaves disk *larger*, because
the delete operations themselves write tombstones (RocksDB) or log/free-list
entries (ParityDB), and neither compacts on reopen. Production pruning must
therefore be paired with **scheduled compaction** (RocksDB) or rely on ParityDB's
background reclamation over time — disk does not shrink the moment you prune.
This applies equally to the Storage Provider's `delete_before` path and is the
in-place-deletion cost the per-bucket-file model sidesteps (see
[report 01, deletion section](01-storage-provider-benchmark.md#decisive-metric-2--bucket-deletion-favors-sharded-decisively)).

### 4. Sustained interleaved load (compaction & memory pressure)

2000 rounds of 100 writes + 100 reads, RSS sampled throughout.

| Engine | throughput (op/s) | peak RSS | RSS delta |
|--------|------------------:|---------:|----------:|
| RocksDB | 937,795 | **93.9 MiB** | ~0 |
| ParityDB | 1,036,422 | 396 MiB | 82 MiB |

Throughput is a near-tie, but **memory behaviour diverges sharply and confirms
the issue's "OS page-cache" concern for ParityDB.** RocksDB holds a flat, bounded
footprint (explicit block cache + small write buffers). ParityDB leans on
mmap/OS page cache and grew to **396 MiB** — over 4× RocksDB. On a node that also
runs networking, the runtime, and (in our topology) shares a host with the
Storage Provider's file-transfer traffic, this page-cache reliance is exactly the
**index-eviction risk** the issue flags. It does not change the recommendation,
but it makes the **cgroup memory isolation** mitigation (Step 2) mandatory rather
than optional.

## Reading

ParityDB wins the metrics that matter for a state-trie backend — cold/warm point-read
latency (by a wide margin), block-import speed, and on-disk compactness — which is
why upstream Substrate offers it as the optimized state backend. Its cost is a
higher, page-cache-driven memory footprint that must be bounded operationally.

The recommendation and justification are in
[03-recommendations.md](03-recommendations.md).
