//! Blockchain Node workloads — a synthetic model of Substrate's state-trie
//! access pattern over the raw engine: random 32-byte keys, batched commits per
//! "block", random point reads, and pruning of old keys.
//!
//! This is deliberately a *microbenchmark on the raw key/value engine*, not a
//! full Substrate node. It isolates the engine's behaviour under the trie
//! access pattern; the faithful end-to-end check (omni-node `--database
//! rocksdb` vs `--database paritydb`) is documented in the report as follow-up.

use super::*;
use crate::engines::Engine;
use crate::metrics::{directory_size_bytes, process_rss_bytes, LatencyStats, Throughput};
use serde_json::json;
use std::time::Instant;

const COMPONENT: &str = "blockchain_node";
const VALUE_BYTES: usize = 64; // representative trie node value

/// Run every state-trie scenario for one engine.
pub fn run_all(engine: Engine, context: &Context) -> Vec<Record> {
    vec![
        block_import(engine, context),
        random_read(engine, context),
        pruning(engine, context),
        sustained(engine, context),
    ]
}

/// Insert `count` random keys, returning the keys in insertion order.
fn populate_keys(
    store: &mut Box<dyn crate::engines::KvStore>,
    count: usize,
    rng: &mut ChaCha8Rng,
) -> Vec<Vec<u8>> {
    let mut keys = Vec::with_capacity(count);
    let mut batch = Vec::with_capacity(1024);
    for _ in 0..count {
        let key = hash_key(rng);
        let value = value_of(rng, VALUE_BYTES);
        keys.push(key.clone());
        batch.push((key, value));
        if batch.len() == 1024 {
            store.commit_batch(&batch, false);
            batch.clear();
        }
    }
    if !batch.is_empty() {
        store.commit_batch(&batch, false);
    }
    store.flush();
    keys
}

/// Block import: durable batched commits, one batch per block.
fn block_import(engine: Engine, context: &Context) -> Record {
    let blocks = context.scaled(1_000);
    let writes_per_block = 200; // state changes per block
    let directory = context.fresh_directory("trie_block_import");
    let mut store = engine.open(&directory);
    let mut rng = ChaCha8Rng::seed_from_u64(context.seed);

    let mut block_durations = Vec::with_capacity(blocks);
    let mut bytes = 0u64;
    let mut ops = 0u64;
    let start = Instant::now();
    for _ in 0..blocks {
        let mut batch = Vec::with_capacity(writes_per_block);
        for _ in 0..writes_per_block {
            let key = hash_key(&mut rng);
            let value = value_of(&mut rng, VALUE_BYTES);
            bytes += (key.len() + value.len()) as u64;
            batch.push((key, value));
            ops += 1;
        }
        let started = Instant::now();
        store.commit_batch(&batch, true); // durable block commit
        block_durations.push(started.elapsed());
    }
    let elapsed = start.elapsed();
    store.flush();
    let disk_bytes = directory_size_bytes(&directory);
    drop(store);

    let mut record = Record::new(COMPONENT, "block_import", engine.name());
    record.params = json!({ "blocks": blocks, "writes_per_block": writes_per_block, "value_bytes": VALUE_BYTES, "sync": true });
    record.throughput = Some(Throughput::new(ops, bytes, elapsed));
    record.latency = Some(LatencyStats::from_durations(block_durations)); // per-block commit
    record.disk_bytes = Some(disk_bytes);
    record
}

/// Random point reads over a large state, cold (after reopen) and warm.
fn random_read(engine: Engine, context: &Context) -> Record {
    let count = context.scaled(1_000_000);
    let reads = context.scaled(100_000);
    let directory = context.fresh_directory("trie_random_read");

    let keys = {
        let mut store = engine.open(&directory);
        let mut rng = ChaCha8Rng::seed_from_u64(context.seed);
        let keys = populate_keys(&mut store, count, &mut rng);
        drop(store);
        keys
    };

    // Reopen for a cold cache.
    let store = engine.open(&directory);
    let mut rng = ChaCha8Rng::seed_from_u64(context.seed ^ 0x5151);
    let pick_key =
        |rng: &mut ChaCha8Rng| -> &Vec<u8> { &keys[(rng.next_u64() as usize) % keys.len()] };

    let cold_count = reads.min(10_000);
    let mut cold = Vec::with_capacity(cold_count);
    for _ in 0..cold_count {
        let key = pick_key(&mut rng);
        let started = Instant::now();
        let got = store.get(key);
        cold.push(started.elapsed());
        assert!(got.is_some(), "missing key during random_read");
    }
    let mut warm = Vec::with_capacity(reads);
    for _ in 0..reads {
        let key = pick_key(&mut rng);
        let started = Instant::now();
        let _ = store.get(key);
        warm.push(started.elapsed());
    }
    drop(store);

    let cold_statistics = LatencyStats::from_durations(cold);
    let mut record = Record::new(COMPONENT, "random_read", engine.name());
    record.params = json!({ "keys": count, "reads": reads });
    record.latency = Some(LatencyStats::from_durations(warm));
    record.extra = json!({ "cold_latency": cold_statistics });
    record
}

/// Pruning: delete the oldest half of the keys, measure delete latency and the
/// disk space reclaimed (after a compaction opportunity).
fn pruning(engine: Engine, context: &Context) -> Record {
    let count = context.scaled(500_000);
    let directory = context.fresh_directory("trie_pruning");
    let mut store = engine.open(&directory);
    let mut rng = ChaCha8Rng::seed_from_u64(context.seed);
    let keys = populate_keys(&mut store, count, &mut rng);
    let disk_before = directory_size_bytes(&directory);

    let prune_count = count / 2;
    let started = Instant::now();
    for key in keys.iter().take(prune_count) {
        store.delete(key);
    }
    store.flush();
    let delete_elapsed = started.elapsed();

    // Give the engine a chance to reclaim (reopen triggers settling; RocksDB
    // tombstones need compaction, surfaced as residual disk).
    drop(store);
    drop(engine.open(&directory));
    let disk_after = directory_size_bytes(&directory);

    let mut record = Record::new(COMPONENT, "pruning", engine.name());
    record.params = json!({ "keys": count, "pruned": prune_count });
    record.throughput = Some(Throughput::new(prune_count as u64, 0, delete_elapsed));
    record.disk_bytes = Some(disk_after);
    record.extra = json!({
        "disk_before_bytes": disk_before,
        "disk_after_bytes": disk_after,
        "reclaimed_fraction": 1.0 - (disk_after as f64 / disk_before.max(1) as f64),
        "delete_total_ms": delete_elapsed.as_secs_f64() * 1e3,
    });
    record
}

/// Sustained interleaved read/write while sampling RSS — surfaces compaction
/// memory pressure over time.
fn sustained(engine: Engine, context: &Context) -> Record {
    let rounds = context.scaled(2_000);
    let writes_per_round = 100;
    let reads_per_round = 100;
    let directory = context.fresh_directory("trie_sustained");
    let mut store = engine.open(&directory);
    let mut rng = ChaCha8Rng::seed_from_u64(context.seed);

    let rss_before = process_rss_bytes();
    let mut max_rss = rss_before.unwrap_or(0);
    let mut keys: Vec<Vec<u8>> = Vec::new();
    let mut ops = 0u64;
    let start = Instant::now();
    for round in 0..rounds {
        let mut batch = Vec::with_capacity(writes_per_round);
        for _ in 0..writes_per_round {
            let key = hash_key(&mut rng);
            keys.push(key.clone());
            batch.push((key, value_of(&mut rng, VALUE_BYTES)));
            ops += 1;
        }
        store.commit_batch(&batch, false);
        if !keys.is_empty() {
            for _ in 0..reads_per_round {
                let key = &keys[(rng.next_u64() as usize) % keys.len()];
                let _ = store.get(key);
                ops += 1;
            }
        }
        if round % 100 == 0 {
            if let Some(rss) = process_rss_bytes() {
                max_rss = max_rss.max(rss);
            }
        }
    }
    let elapsed = start.elapsed();
    store.flush();
    drop(store);

    let mut record = Record::new(COMPONENT, "sustained", engine.name());
    record.params = json!({ "rounds": rounds, "writes_per_round": writes_per_round, "reads_per_round": reads_per_round });
    record.throughput = Some(Throughput::new(ops, 0, elapsed));
    record.rss_delta_bytes = rss_before.map(|before| max_rss.saturating_sub(before));
    record.extra = json!({ "peak_rss_bytes": max_rss });
    record
}
