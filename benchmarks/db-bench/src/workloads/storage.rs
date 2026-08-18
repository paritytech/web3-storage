//! Storage Provider workloads — model the post-Issue-#100 per-bucket database:
//! each bucket is one small, independent store keyed by MMR leaf position.

use super::*;
use crate::engines::Engine;
use crate::metrics::{
    directory_size_bytes, open_fd_count, process_rss_bytes, LatencyStats, Throughput,
};
use serde_json::json;
use std::time::Instant;

const COMPONENT: &str = "storage_provider";

/// Run every Storage Provider scenario for one engine.
pub fn run_all(engine: Engine, context: &Context) -> Vec<Record> {
    vec![
        open_close(engine, context),
        mmr_append(engine, context, 48, "mmr_append_small"),
        mmr_append(engine, context, 256 * 1024, "node_append_large"),
        proof_read(engine, context),
        disk_efficiency(engine, context, 48, "disk_small"),
        disk_efficiency(engine, context, 256 * 1024, "disk_large"),
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
    record.disk_bytes = Some(directory_size_bytes(&directory));
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
    let on_disk = directory_size_bytes(&directory);

    // Steady-state size is what an engine costs day to day, but an engine with
    // a reclamation API can be scheduled to give space back. Measure both so
    // the two are not conflated: copy-on-write and log-structured engines look
    // far worse before compaction than after.
    let mut store = engine.open(&directory);
    let has_compaction = store.compact();
    drop(store);
    let on_disk_compacted = directory_size_bytes(&directory);

    let mut record = Record::new(COMPONENT, scenario, engine.name());
    record.params = json!({ "entries": count, "value_bytes": value_size });
    record.disk_bytes = Some(on_disk);
    record.extra = json!({
        "logical_bytes": logical,
        "amplification": on_disk as f64 / logical.max(1) as f64,
        "has_compaction_api": has_compaction,
        "disk_compacted_bytes": on_disk_compacted,
        "amplification_compacted": on_disk_compacted as f64 / logical.max(1) as f64,
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
    let size_after_delete = directory_size_bytes(&directory_a);

    // Deleting keys writes tombstones/free-list entries rather than freeing
    // bytes; time the compaction that actually reclaims them, since production
    // pruning has to pay this cost somewhere.
    let compaction_started = Instant::now();
    let has_compaction = store.compact();
    let compaction_elapsed = compaction_started.elapsed();
    drop(store);
    let size_after_compaction = directory_size_bytes(&directory_a);

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
        record.extra = json!({
            "open_total_ms": open_elapsed.as_secs_f64() * 1e3,
            "open_per_instance_us": (open_elapsed.as_secs_f64() * 1e6) / count as f64,
            "fd_per_instance": record.fd_delta.map(|fd| fd as f64 / count as f64),
        });

        // Close all instances before the next count.
        drop(stores);
        let _ = std::fs::remove_dir_all(&base);
        records.push(record);
    }
    records
}
