//! Storage Provider workloads — model the two per-bucket stores of
//! `05-per-bucket-store-design.md`:
//!
//! - **Commitment store** — 48-byte `MmrLeaf` values under dense position keys,
//!   fully durable per transaction. Modelled by `mmr_append_small`, `proof_read`,
//!   `disk_small`, and the reopen/instance-scaling scenarios.
//! - **Content store** — 256 KiB chunks under random content-hash keys, ingested
//!   in unsynced batches behind a single flush barrier. Modelled by
//!   `content_store`.
//!
//! Earlier passes also ran `node_append_large` and `disk_large`: 256 KiB values
//! under *sequential position keys with a per-batch fsync*. That combination
//! exists nowhere in the design — chunks are hash-keyed with barrier durability,
//! and position-keyed values are 48 bytes — so those scenarios were retired once
//! `content_store` was added. Their historical figures remain in the pass-1..4
//! result files.

use super::*;
use crate::engines::Engine;
use crate::metrics::{
    directory_allocated_bytes, directory_size_bytes, open_fd_count, process_rss_bytes,
    process_thread_count, process_vsize_bytes, write_ahead_bytes, LatencyStats, Throughput,
};
use serde_json::json;
use std::time::Instant;

const COMPONENT: &str = "storage_provider";

/// Run every Storage Provider scenario for one engine.
pub fn run_all(engine: Engine, context: &Context) -> Vec<Record> {
    vec![
        empty_floor(engine, context),
        open_close(engine, context),
        mmr_append(engine, context, 48, "mmr_append_small"),
        proof_read(engine, context),
        disk_efficiency(engine, context, 48, "disk_small"),
        content_store(engine, context),
        bulk_delete(engine, context),
    ]
    .into_iter()
    .chain(multi_instance(engine, context))
    .collect()
}

/// Populate a store at `path` with `count` position-keyed leaves and flush.
fn populate(engine: Engine, path: &std::path::Path, count: usize, value_size: usize, seed: u64) {
    let mut store = engine.open(path);
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let mut batch = Vec::with_capacity(256);
    for position in 0..count as u64 {
        batch.push((position_key(position), value_of(&mut rng, value_size)));
        if batch.len() == 256 {
            store.commit_batch(&batch, false);
            batch.clear();
        }
    }
    if !batch.is_empty() {
        store.commit_batch(&batch, false);
    }
    store.flush();
}

/// Dedup-lookup probe — tests *why* an absent-key lookup is slow, not just that
/// it is.
///
/// Pass 5 measured SQLite answering a dedup miss in 746 µs, slower than its
/// present-key read and ~1500× slower than LMDB. The hypothesis was that the
/// cost is not intrinsic to the miss but comes from the un-checkpointed WAL that
/// bulk ingest accumulates: every read must consult the write-ahead set before
/// the B-tree, and that set grows with everything written since the last
/// checkpoint. If true, the fix is a checkpoint policy, not a different engine.
///
/// The probe samples miss latency and WAL size together as ingest proceeds, in
/// two modes:
///
/// - `flush_every: None` — the pass-5 behaviour: ingest with no intermediate
///   checkpoint, so the write-ahead set grows monotonically.
/// - `flush_every: Some(n)` — checkpoint every `n` batches, holding the
///   write-ahead set small.
///
/// If the hypothesis holds, mode 1 shows miss latency rising with WAL bytes and
/// mode 2 shows it flat and low. If miss latency is high in both, the cost is
/// intrinsic and the engine comparison stands as measured.
fn dedup_probe(
    engine: Engine,
    context: &Context,
    flush_every: Option<usize>,
    count: usize,
    value_size: usize,
) -> Record {
    let count = context.scaled(count);
    let batch_size = 16;
    let sample_every_batches = 8;
    let probes_per_sample = 21; // odd, so the median is a real observation

    let tag = match flush_every {
        Some(n) => format!("dedup_probe_flush{n}"),
        None => "dedup_probe_noflush".to_string(),
    };
    let tag = format!("{tag}_v{value_size}_n{count}");
    let directory = context.fresh_directory(&tag);
    let mut store = engine.open(&directory);
    let mut rng = ChaCha8Rng::seed_from_u64(context.seed ^ 0xDED0);

    // Keys used only for probing; never written, so every lookup is a true miss.
    let mut probe_rng = ChaCha8Rng::seed_from_u64(context.seed ^ 0x9999);
    let sample_miss_us = |store: &dyn crate::engines::KvStore, probe_rng: &mut ChaCha8Rng| -> f64 {
        let mut samples = Vec::with_capacity(probes_per_sample);
        for _ in 0..probes_per_sample {
            let mut key = vec![0u8; 32];
            probe_rng.fill_bytes(&mut key);
            let started = Instant::now();
            let found = store.get(&key);
            samples.push(started.elapsed());
            debug_assert!(found.is_none(), "probe key must never be present");
        }
        samples.sort_unstable();
        samples[probes_per_sample / 2].as_secs_f64() * 1e6
    };

    let mut series = Vec::new();
    let mut written = 0usize;
    let mut batch_index = 0usize;
    let mut batch = Vec::with_capacity(batch_size);

    // Baseline before anything is written: the intrinsic cost on an empty store.
    series.push(json!({
        "chunks_written": 0,
        "wal_bytes": write_ahead_bytes(&directory),
        "miss_p50_us": sample_miss_us(&*store, &mut probe_rng),
    }));

    while written < count {
        for _ in 0..batch_size {
            if written >= count {
                break;
            }
            let mut key = vec![0u8; 32];
            rng.fill_bytes(&mut key);
            batch.push((key, value_of(&mut rng, value_size)));
            written += 1;
        }
        store.commit_batch(&batch, false);
        batch.clear();
        batch_index += 1;

        if let Some(n) = flush_every {
            if batch_index.is_multiple_of(n) {
                store.flush();
            }
        }
        if batch_index.is_multiple_of(sample_every_batches) {
            series.push(json!({
                "chunks_written": written,
                "wal_bytes": write_ahead_bytes(&directory),
                "miss_p50_us": sample_miss_us(&*store, &mut probe_rng),
            }));
        }
    }

    // Quiesced: everything checkpointed, nothing pending. This is the intrinsic
    // absent-key cost with no write-ahead set to search.
    store.flush();
    let quiesced_wal = write_ahead_bytes(&directory);
    let quiesced_miss = sample_miss_us(&*store, &mut probe_rng);
    drop(store);

    let peak_wal = series
        .iter()
        .filter_map(|s| s["wal_bytes"].as_u64())
        .max()
        .unwrap_or(0);
    let peak_miss = series
        .iter()
        .filter_map(|s| s["miss_p50_us"].as_f64())
        .fold(0.0f64, f64::max);

    let mut record = Record::new(COMPONENT, &tag, engine.name());
    record.params = json!({
        "entries": count,
        "value_bytes": value_size,
        "batch_size": batch_size,
        "flush_every_batches": flush_every,
    });
    record.disk_bytes = Some(directory_allocated_bytes(&directory));
    record.extra = json!({
        "series": series,
        "peak_wal_bytes": peak_wal,
        "peak_miss_p50_us": peak_miss,
        "quiesced_wal_bytes": quiesced_wal,
        "quiesced_miss_p50_us": quiesced_miss,
    });
    record
}

/// Run the dedup probe.
///
/// The three shapes separate the two candidate explanations for a slow miss:
/// whether cost follows the **bytes** stored or the **number of keys**. Holding
/// the key count fixed while shrinking the value isolates the first; holding
/// bytes roughly fixed while raising the key count 50× isolates the second.
pub fn run_dedup_probe(engine: Engine, context: &Context) -> Vec<Record> {
    let shapes = [
        (2_000, 256 * 1024), // as the content store really is
        (2_000, 48),         // same keys, ~5000× fewer bytes
        (100_000, 48),       // 50× the keys, still few bytes
    ];
    let mut records = Vec::new();
    for (count, value_size) in shapes {
        records.push(dedup_probe(engine, context, None, count, value_size));
        records.push(dedup_probe(engine, context, Some(8), count, value_size));
    }
    records
}

/// Cost of a bucket that exists but holds nothing.
///
/// Every provisioned bucket pays this before it stores a single byte, so at a
/// million buckets it is a floor on total disk regardless of utilisation. Both
/// numbers are reported because they differ by orders of magnitude for engines
/// that preallocate a sparse map: LMDB's file *length* is its whole `map_size`,
/// while its allocated blocks are a few pages.
fn empty_floor(engine: Engine, context: &Context) -> Record {
    let directory = context.fresh_directory("provider_empty_floor");

    let create_started = Instant::now();
    let mut store = engine.open(&directory);
    store.flush();
    let create_elapsed = create_started.elapsed();
    drop(store);

    let allocated = directory_allocated_bytes(&directory);
    let apparent = directory_size_bytes(&directory);
    let files = count_files(&directory);

    let mut record = Record::new(COMPONENT, "empty_floor", engine.name());
    record.params = json!({ "entries": 0 });
    record.disk_bytes = Some(allocated);
    record.extra = json!({
        "create_us": create_elapsed.as_secs_f64() * 1e6,
        "disk_apparent_bytes": apparent,
        "files_per_instance": files,
        // What a million provisioned-but-empty buckets would cost on disk.
        "projected_gib_at_1m_buckets": (allocated as f64 * 1_000_000.0) / (1024.0 * 1024.0 * 1024.0),
    });
    record
}

/// Number of regular files an engine leaves on disk for one store.
fn count_files(path: &std::path::Path) -> u64 {
    fn walk(path: &std::path::Path, total: &mut u64) {
        let Ok(metadata) = std::fs::symlink_metadata(path) else {
            return;
        };
        if metadata.is_file() {
            *total += 1;
        } else if metadata.is_dir() {
            if let Ok(entries) = std::fs::read_dir(path) {
                for entry in entries.flatten() {
                    walk(&entry.path(), total);
                }
            }
        }
    }
    let mut total = 0;
    walk(path, &mut total);
    total
}

/// Open/close latency: reopen of an already-populated bucket DB, the cost an
/// LRU connection pool pays on eviction + reload.
fn open_close(engine: Engine, context: &Context) -> Record {
    let leaf_count = context.scaled(1024);
    let iterations = context.scaled(50).clamp(5, 50);
    let directory = context.fresh_directory("provider_open_close");
    populate(engine, &directory, leaf_count, 48, context.seed);

    let mut durations = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let started = Instant::now();
        let store = engine.open(&directory);
        durations.push(started.elapsed());
        drop(store); // close releases locks/handles
    }

    let mut record = Record::new(COMPONENT, "open_close", engine.name());
    record.params = json!({ "preloaded_leaves": leaf_count, "reopen_iters": iterations });
    record.latency = Some(LatencyStats::from_durations(durations));
    record.extra = json!({ "note": "latency = time to reopen an existing populated bucket DB" });
    record
}

/// MMR append throughput: durable batched writes of position-keyed entries.
/// `value_size` = 48 models leaf metadata; 256 KiB models full chunk nodes.
fn mmr_append(engine: Engine, context: &Context, value_size: usize, scenario: &str) -> Record {
    let total = if value_size <= 64 {
        context.scaled(100_000)
    } else {
        context.scaled(2_000) // large values: fewer ops
    };
    let batch_size = 16; // a checkpoint's worth of node positions
    let directory = context.fresh_directory(scenario);
    let mut store = engine.open(&directory);
    let mut rng = ChaCha8Rng::seed_from_u64(context.seed);

    let mut batch_durations = Vec::new();
    let mut bytes = 0u64;
    let start = Instant::now();
    let mut position = 0u64;
    while (position as usize) < total {
        let mut batch = Vec::with_capacity(batch_size);
        for _ in 0..batch_size {
            if (position as usize) >= total {
                break;
            }
            let value = value_of(&mut rng, value_size);
            bytes += (value.len() + 8) as u64;
            batch.push((position_key(position), value));
            position += 1;
        }
        let started = Instant::now();
        store.commit_batch(&batch, true); // durable checkpoint
        batch_durations.push(started.elapsed());
    }
    let elapsed = start.elapsed();
    store.flush();
    drop(store);

    let mut record = Record::new(COMPONENT, scenario, engine.name());
    record.params = json!({ "entries": total, "value_bytes": value_size, "batch_size": batch_size, "sync": true });
    record.throughput = Some(Throughput::new(position, bytes, elapsed));
    record.latency = Some(LatencyStats::from_durations(batch_durations)); // per durable batch
    record.disk_bytes = Some(directory_allocated_bytes(&directory));
    record
}

/// Random proof reads: lookups by position over a populated, reopened DB.
fn proof_read(engine: Engine, context: &Context) -> Record {
    let leaf_count = context.scaled(100_000);
    let reads = context.scaled(50_000);
    let directory = context.fresh_directory("provider_proof_read");
    populate(engine, &directory, leaf_count, 48, context.seed);

    // Reopen for a cold-ish cache, then measure warm random reads.
    let store = engine.open(&directory);
    let mut rng = ChaCha8Rng::seed_from_u64(context.seed ^ 0xABCD);

    // Cold phase (first pass).
    let mut cold = Vec::with_capacity(reads.min(5_000));
    for _ in 0..reads.min(5_000) {
        let position = rng.next_u64() % leaf_count as u64;
        let started = Instant::now();
        let got = store.get(&position_key(position));
        cold.push(started.elapsed());
        assert!(got.is_some(), "missing key during proof_read");
    }
    // Warm phase.
    let mut warm = Vec::with_capacity(reads);
    for _ in 0..reads {
        let position = rng.next_u64() % leaf_count as u64;
        let started = Instant::now();
        let _ = store.get(&position_key(position));
        warm.push(started.elapsed());
    }
    drop(store);

    let cold_statistics = LatencyStats::from_durations(cold);
    let mut record = Record::new(COMPONENT, "proof_read", engine.name());
    record.params = json!({ "leaves": leaf_count, "reads": reads });
    record.latency = Some(LatencyStats::from_durations(warm)); // steady state
    record.extra = json!({ "cold_latency": cold_statistics });
    record
}

/// Content-store scenario — the per-bucket chunk store as designed: values are
/// 256 KiB chunks keyed by **content hash**, so keys arrive in *random* order
/// (unlike every other scenario's sequential positions — this is what defeats
/// B-tree sequential-insert packing). Ingest is **unsynced** batches followed
/// by a single `flush()`, modelling the durability barrier the provider pays
/// once, immediately before committing the MMR and signing: chunk writes are
/// idempotent and content-verified, so per-batch fsyncs buy nothing. Reads are
/// random point lookups — the chunk-serving path for downloads, client
/// spot-checks, and challenges — plus the absent-key dedup lookup each upload
/// performs before writing (`miss_latency`).
fn content_store(engine: Engine, context: &Context) -> Record {
    let count = context.scaled(2_000);
    let batch_size = 16;
    let value_size = 256 * 1024;
    let directory = context.fresh_directory("provider_content_store");
    let mut store = engine.open(&directory);
    let mut rng = ChaCha8Rng::seed_from_u64(context.seed ^ 0x5EED);

    // Random 32-byte keys stand in for blake2 content hashes: same size, same
    // uniform distribution, no extra dependency.
    let mut keys: Vec<Vec<u8>> = Vec::with_capacity(count);
    let mut bytes = 0u64;
    let mut batch = Vec::with_capacity(batch_size);
    // Every real upload calls `check_exists` before storing a chunk, and for new
    // content that lookup *misses*. Absent-key cost is not the same as present-key
    // cost — an LSM answers from a bloom filter without touching a table, while a
    // B-tree still descends to the leaf — so it is measured separately here and
    // subtracted from the ingest window, which keeps ingest throughput comparable
    // with passes that had no such check.
    let mut miss = Vec::with_capacity(count);
    let mut miss_total = std::time::Duration::ZERO;
    let ingest_started = Instant::now();
    for _ in 0..count {
        let mut key = vec![0u8; 32];
        rng.fill_bytes(&mut key);

        let miss_started = Instant::now();
        let existing = store.get(&key);
        let miss_elapsed = miss_started.elapsed();
        miss_total += miss_elapsed;
        miss.push(miss_elapsed);
        debug_assert!(
            existing.is_none(),
            "fresh content hash must not already exist"
        );

        let value = value_of(&mut rng, value_size);
        bytes += (key.len() + value.len()) as u64;
        keys.push(key.clone());
        batch.push((key, value));
        if batch.len() == batch_size {
            store.commit_batch(&batch, false); // relaxed: no per-batch fsync
            batch.clear();
        }
    }
    if !batch.is_empty() {
        store.commit_batch(&batch, false);
    }
    let ingest_elapsed = ingest_started.elapsed().saturating_sub(miss_total);

    // The barrier: one durable flush before the commitment store would commit.
    let barrier_started = Instant::now();
    store.flush();
    let barrier = barrier_started.elapsed();
    drop(store);

    // Reopen for a cold cache, then measure the chunk-serving read path.
    let store = engine.open(&directory);
    let cold_reads = count.min(1_000);
    let mut cold = Vec::with_capacity(cold_reads);
    for _ in 0..cold_reads {
        let key = &keys[(rng.next_u64() as usize) % keys.len()];
        let started = Instant::now();
        let got = store.get(key);
        cold.push(started.elapsed());
        assert!(got.is_some(), "missing chunk during content_store read");
    }
    let warm_reads = context.scaled(5_000);
    let mut warm = Vec::with_capacity(warm_reads);
    for _ in 0..warm_reads {
        let key = &keys[(rng.next_u64() as usize) % keys.len()];
        let started = Instant::now();
        let _ = store.get(key);
        warm.push(started.elapsed());
    }
    drop(store);

    let logical = bytes;
    // Allocated blocks, not apparent length: engines that map a large sparse
    // file (LMDB, redb, mdbx) would otherwise report their declared ceiling as
    // their footprint and produce a meaningless amplification figure.
    let on_disk = directory_allocated_bytes(&directory);
    let on_disk_apparent = directory_size_bytes(&directory);
    let mut record = Record::new(COMPONENT, "content_store", engine.name());
    record.params = json!({
        "entries": count,
        "value_bytes": value_size,
        "batch_size": batch_size,
        "sync": false,
        "keying": "random 32-byte (content hash)",
    });
    record.throughput = Some(Throughput::new(count as u64, bytes, ingest_elapsed));
    record.latency = Some(LatencyStats::from_durations(warm)); // chunk serving, steady state
    record.disk_bytes = Some(on_disk);
    record.extra = json!({
        "cold_latency": LatencyStats::from_durations(cold),
        // Absent-key lookup: the dedup check every upload pays before storing.
        "miss_latency": LatencyStats::from_durations(miss),
        "barrier_us": barrier.as_secs_f64() * 1e6,
        "logical_bytes": logical,
        "amplification": on_disk as f64 / logical.max(1) as f64,
        "disk_apparent_bytes": on_disk_apparent,
    });
    record
}

/// Disk space efficiency: on-disk bytes vs logical payload for `count` entries.
fn disk_efficiency(engine: Engine, context: &Context, value_size: usize, scenario: &str) -> Record {
    let count = if value_size <= 64 {
        context.scaled(100_000)
    } else {
        context.scaled(2_000)
    };
    let directory = context.fresh_directory(scenario);
    populate(engine, &directory, count, value_size, context.seed);
    // Reopen + drop to force any background flush/compaction to settle.
    drop(engine.open(&directory));

    let logical = (count * (value_size + 8)) as u64;
    let on_disk = directory_allocated_bytes(&directory);
    let on_disk_apparent = directory_size_bytes(&directory);

    // Steady-state size is what an engine costs day to day, but an engine with
    // a reclamation API can be scheduled to give space back. Measure both so
    // the two are not conflated: copy-on-write and log-structured engines look
    // far worse before compaction than after.
    let mut store = engine.open(&directory);
    let has_compaction = store.compact();
    drop(store);
    let on_disk_compacted = directory_allocated_bytes(&directory);

    let mut record = Record::new(COMPONENT, scenario, engine.name());
    record.params = json!({ "entries": count, "value_bytes": value_size });
    record.disk_bytes = Some(on_disk);
    record.extra = json!({
        "logical_bytes": logical,
        "amplification": on_disk as f64 / logical.max(1) as f64,
        "has_compaction_api": has_compaction,
        "disk_compacted_bytes": on_disk_compacted,
        "amplification_compacted": on_disk_compacted as f64 / logical.max(1) as f64,
        // Apparent size counts a sparse preallocated map at its declared
        // ceiling; `disk_bytes` above is the allocated-blocks truth.
        "disk_apparent_bytes": on_disk_apparent,
    });
    record
}

/// Bulk deletion: clearing a whole bucket. Two strategies are timed —
/// in-engine delete of every key (the shared-DB / tombstone model) vs. simply
/// removing the directory (the per-bucket-file model).
fn bulk_delete(engine: Engine, context: &Context) -> Record {
    let count = context.scaled(50_000);

    // Strategy A: delete every key inside the engine.
    let directory_a = context.fresh_directory("provider_delete_keys");
    populate(engine, &directory_a, count, 48, context.seed);
    let mut store = engine.open(&directory_a);
    let started = Instant::now();
    for position in 0..count as u64 {
        store.delete(&position_key(position));
    }
    store.flush();
    let delete_keys_elapsed = started.elapsed();
    let size_after_delete = directory_allocated_bytes(&directory_a);

    // Deleting keys writes tombstones/free-list entries rather than freeing
    // bytes; time the compaction that actually reclaims them, since production
    // pruning has to pay this cost somewhere.
    let compaction_started = Instant::now();
    let has_compaction = store.compact();
    let compaction_elapsed = compaction_started.elapsed();
    drop(store);
    let size_after_compaction = directory_allocated_bytes(&directory_a);

    // Strategy B: drop the store and remove the directory (file-level cleanup).
    let directory_b = context.fresh_directory("provider_delete_rmtree");
    populate(engine, &directory_b, count, 48, context.seed);
    let remove_elapsed = remove_tree_timed(&directory_b);

    let mut record = Record::new(COMPONENT, "bulk_delete", engine.name());
    record.params = json!({ "entries": count });
    record.extra = json!({
        "delete_all_keys_us": delete_keys_elapsed.as_secs_f64() * 1e6,
        "bytes_after_key_delete": size_after_delete,
        "has_compaction_api": has_compaction,
        "compaction_us": compaction_elapsed.as_secs_f64() * 1e6,
        "bytes_after_compaction": size_after_compaction,
        "rmtree_us": remove_elapsed.as_secs_f64() * 1e6,
        "note": "rmtree models per-bucket-file cleanup; delete_all_keys models shared-DB tombstones, \
                 whose space is only returned by the subsequent compaction",
    });
    record
}

/// File-descriptor and memory scaling: open N populated instances at once.
fn multi_instance(engine: Engine, context: &Context) -> Vec<Record> {
    let counts = [100usize, 500, 1000];
    let per_instance_leaves = context.scaled(64).max(8);
    let mut records = Vec::new();

    for &count in &counts {
        let count = context.scaled(count).max(1);
        let base = context.fresh_directory(&format!("provider_multi_{count}"));

        let fd_before = open_fd_count();
        let rss_before = process_rss_bytes();
        let threads_before = process_thread_count();
        let vsize_before = process_vsize_bytes();

        let mut stores = Vec::with_capacity(count);
        let open_started = Instant::now();
        for i in 0..count {
            let directory = base.join(format!("b{i}"));
            std::fs::create_dir_all(&directory).expect("mkdir instance");
            let mut store = engine.open(&directory);
            let mut rng = ChaCha8Rng::seed_from_u64(context.seed ^ i as u64);
            let batch: Vec<_> = (0..per_instance_leaves as u64)
                .map(|position| (position_key(position), value_of(&mut rng, 48)))
                .collect();
            store.commit_batch(&batch, false);
            store.flush();
            stores.push(store);
        }
        let open_elapsed = open_started.elapsed();

        // Sample with all instances live.
        let fd_after = open_fd_count();
        let rss_after = process_rss_bytes();
        let threads_after = process_thread_count();
        let vsize_after = process_vsize_bytes();

        let mut record = Record::new(COMPONENT, "multi_instance", engine.name());
        record.params = json!({ "instances": count, "leaves_per_instance": per_instance_leaves });
        record.fd_delta = match (fd_before, fd_after) {
            (Some(before), Some(after)) => Some(after.saturating_sub(before)),
            _ => None,
        };
        record.rss_delta_bytes = match (rss_before, rss_after) {
            (Some(before), Some(after)) => Some(after.saturating_sub(before)),
            _ => None,
        };
        // Threads and address space are the two per-instance costs the earlier
        // reports never measured: background-compaction engines multiply the
        // former by every open instance, and mmap engines that reserve a fixed
        // map per instance exhaust the latter long before they exhaust RAM.
        let thread_delta = match (threads_before, threads_after) {
            (Some(before), Some(after)) => Some(after.saturating_sub(before)),
            _ => None,
        };
        let vsize_delta = match (vsize_before, vsize_after) {
            (Some(before), Some(after)) => Some(after.saturating_sub(before)),
            _ => None,
        };
        record.extra = json!({
            "open_total_ms": open_elapsed.as_secs_f64() * 1e3,
            "open_per_instance_us": (open_elapsed.as_secs_f64() * 1e6) / count as f64,
            "fd_per_instance": record.fd_delta.map(|fd| fd as f64 / count as f64),
            "threads_added": thread_delta,
            "threads_per_instance": thread_delta.map(|t| t as f64 / count as f64),
            "vsize_added_bytes": vsize_delta,
            "vsize_per_instance_bytes": vsize_delta.map(|v| v as f64 / count as f64),
            "rss_per_instance_bytes": record
                .rss_delta_bytes
                .map(|rss| rss as f64 / count as f64),
        });

        // Close all instances before the next count.
        drop(stores);
        let _ = std::fs::remove_dir_all(&base);
        records.push(record);
    }
    records
}
