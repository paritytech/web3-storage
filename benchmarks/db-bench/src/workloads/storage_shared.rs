//! Storage Provider — **shared-DB** architecture (one database holding every
//! bucket, keyed by `bucket_id || position`). The counterpart to the sharded
//! (per-bucket-file) scenarios in `storage.rs`. Together they let Issue #100
//! decide sharded vs shared with measured numbers rather than assumption.

use super::*;
use crate::engines::{open_shared_concurrent, Engine};
use crate::metrics::{
    directory_allocated_bytes, open_fd_count, process_rss_bytes, LatencyStats, Throughput,
};
use serde_json::json;
use std::time::Instant;

const COMPONENT: &str = "storage_provider";

/// Single-instance shared-DB scenarios for one engine.
pub fn run_all(engine: Engine, context: &Context) -> Vec<Record> {
    let mut records = vec![shared_write_read(engine, context)];
    records.extend(shared_read(engine, context));
    records.push(shared_bucket_delete(engine, context));
    records
}

/// Populate one DB with `buckets × leaves` entries and measure write
/// throughput plus the single instance's resident memory, FDs, and disk —
/// directly comparable to the sharded `multi_instance` scenario at the same
/// bucket count.
fn shared_write_read(engine: Engine, context: &Context) -> Record {
    let buckets = context.scaled(1000);
    let leaves = context.scaled(64).max(8);
    let directory = context.fresh_directory("shared_write");

    let fd_before = open_fd_count();
    let rss_before = process_rss_bytes();

    let mut store = engine.open(&directory);
    let mut rng = ChaCha8Rng::seed_from_u64(context.seed);
    let mut batch_durations = Vec::new();
    let mut ops = 0u64;
    let mut bytes = 0u64;
    let start = Instant::now();
    for bucket in 0..buckets as u64 {
        // One bucket's leaves committed as a durable batch (a checkpoint).
        let mut batch = Vec::with_capacity(leaves);
        for position in 0..leaves as u64 {
            let value = value_of(&mut rng, 48);
            bytes += (value.len() + 16) as u64;
            batch.push((shared_key(bucket, position), value));
            ops += 1;
        }
        let started = Instant::now();
        store.commit_batch(&batch, true);
        batch_durations.push(started.elapsed());
    }
    let elapsed = start.elapsed();
    store.flush();

    let fd_after = open_fd_count();
    let rss_after = process_rss_bytes();
    let disk_bytes = directory_allocated_bytes(&directory);
    drop(store);

    let mut record = Record::new(COMPONENT, "shared_write", engine.name());
    record.params =
        json!({ "architecture": "shared", "buckets": buckets, "leaves_per_bucket": leaves });
    record.throughput = Some(Throughput::new(ops, bytes, elapsed));
    record.latency = Some(LatencyStats::from_durations(batch_durations)); // per-bucket checkpoint
    record.disk_bytes = Some(disk_bytes);
    record.fd_delta = match (fd_before, fd_after) {
        (Some(before), Some(after)) => Some(after.saturating_sub(before)),
        _ => None,
    };
    record.rss_delta_bytes = match (rss_before, rss_after) {
        (Some(before), Some(after)) => Some(after.saturating_sub(before)),
        _ => None,
    };
    record
}

/// Random proof reads over the shared DB: pick a random `(bucket, position)`.
fn shared_read(engine: Engine, context: &Context) -> Vec<Record> {
    let buckets = context.scaled(1000);
    let leaves = context.scaled(64).max(8);
    let reads = context.scaled(50_000);
    let directory = context.fresh_directory("shared_read");

    // Populate.
    {
        let mut store = engine.open(&directory);
        let mut rng = ChaCha8Rng::seed_from_u64(context.seed);
        for bucket in 0..buckets as u64 {
            let mut batch = Vec::with_capacity(leaves);
            for position in 0..leaves as u64 {
                batch.push((shared_key(bucket, position), value_of(&mut rng, 48)));
            }
            store.commit_batch(&batch, false);
        }
        store.flush();
    }

    // Reopen (cold cache) and read.
    let store = engine.open(&directory);
    let mut rng = ChaCha8Rng::seed_from_u64(context.seed ^ 0x7777);
    let mut warm = Vec::with_capacity(reads);
    for _ in 0..reads {
        let bucket = rng.next_u64() % buckets as u64;
        let position = rng.next_u64() % leaves as u64;
        let started = Instant::now();
        let _ = store.get(&shared_key(bucket, position));
        warm.push(started.elapsed());
    }
    drop(store);

    let mut record = Record::new(COMPONENT, "shared_proof_read", engine.name());
    record.params = json!({ "architecture": "shared", "buckets": buckets, "leaves_per_bucket": leaves, "reads": reads });
    record.latency = Some(LatencyStats::from_durations(warm));
    vec![record]
}

/// Bulk bucket deletion in the shared model: clearing many expired buckets
/// means deleting all their keys in place (tombstones), the spike the issue
/// flags. Contrast with the sharded model's single-file `unlink` (~0.1 ms,
/// 100% reclaim — see `storage.rs::bulk_delete`).
fn shared_bucket_delete(engine: Engine, context: &Context) -> Record {
    let buckets = context.scaled(1000);
    let leaves = context.scaled(64).max(8);
    let to_delete = buckets / 2; // clear half the buckets (expired agreements)
    let directory = context.fresh_directory("shared_delete");

    let mut store = engine.open(&directory);
    let mut rng = ChaCha8Rng::seed_from_u64(context.seed);
    for bucket in 0..buckets as u64 {
        let mut batch = Vec::with_capacity(leaves);
        for position in 0..leaves as u64 {
            batch.push((shared_key(bucket, position), value_of(&mut rng, 48)));
        }
        store.commit_batch(&batch, false);
    }
    store.flush();
    let disk_before = directory_allocated_bytes(&directory);

    // Delete the first `to_delete` buckets, key by key (no native range delete
    // for hash-indexed engines; the contiguous key layout from Step 4 is what
    // would make a range delete possible where supported).
    let started = Instant::now();
    for bucket in 0..to_delete as u64 {
        for position in 0..leaves as u64 {
            store.delete(&shared_key(bucket, position));
        }
    }
    store.flush();
    let delete_elapsed = started.elapsed();
    drop(store);
    drop(engine.open(&directory)); // settle
    let disk_after = directory_allocated_bytes(&directory);

    let mut record = Record::new(COMPONENT, "shared_bucket_delete", engine.name());
    record.params = json!({ "architecture": "shared", "buckets": buckets, "deleted_buckets": to_delete, "leaves_per_bucket": leaves });
    record.throughput = Some(Throughput::new(
        (to_delete * leaves) as u64,
        0,
        delete_elapsed,
    ));
    record.disk_bytes = Some(disk_after);
    record.extra = json!({
        "delete_total_ms": delete_elapsed.as_secs_f64() * 1e3,
        "disk_before_bytes": disk_before,
        "disk_after_bytes": disk_after,
        "reclaimed_fraction": 1.0 - (disk_after as f64 / disk_before.max(1) as f64),
        "note": "shared-DB bucket clear = in-place key deletes; sharded equivalent is a file unlink (~0.1 ms, full reclaim)",
    });
    record
}

/// Whether concurrent writers hit one shared DB or one DB per thread.
#[derive(Clone, Copy)]
pub enum Architecture {
    Shared,
    Sharded,
}

impl Architecture {
    fn label(self) -> &'static str {
        match self {
            Architecture::Shared => "shared",
            Architecture::Sharded => "sharded",
        }
    }
}

fn thread_count() -> usize {
    std::thread::available_parallelism()
        .map(|parallelism| parallelism.get())
        .unwrap_or(4)
        .clamp(2, 8)
}

/// Concurrent multi-bucket writes — the decisive sharded-vs-shared test.
///
/// `T` threads each write a disjoint bucket's leaves. In `Shared`, all threads
/// write into one database (SQLite serializes on its single writer lock; the
/// others are internally concurrent). In `Sharded`, each thread owns its own
/// per-bucket database file, so writes are fully parallel for every engine.
pub fn concurrent_write(engine: Engine, context: &Context, architecture: Architecture) -> Record {
    let threads = thread_count();
    let per_thread = context.scaled(20_000).max(100);
    let batch_size = 16;
    let base = context.fresh_directory(&format!(
        "concurrent_{}_{}",
        architecture.label(),
        engine.name()
    ));

    let total_ops = (threads * per_thread) as u64;
    let start = Instant::now();

    match architecture {
        Architecture::Shared => {
            let shared = open_shared_concurrent(engine, &base);
            std::thread::scope(|scope| {
                for thread_id in 0..threads {
                    let shared = shared.clone();
                    let seed = context.seed ^ (thread_id as u64).wrapping_mul(0x9E3779B97F4A7C15);
                    scope.spawn(move || {
                        let mut writer = shared.new_writer();
                        write_bucket(&mut *writer, thread_id as u64, per_thread, batch_size, seed);
                    });
                }
            });
        }
        Architecture::Sharded => {
            std::thread::scope(|scope| {
                for thread_id in 0..threads {
                    let directory = base.join(format!("b{thread_id}"));
                    let seed = context.seed ^ (thread_id as u64).wrapping_mul(0x9E3779B97F4A7C15);
                    scope.spawn(move || {
                        std::fs::create_dir_all(&directory).expect("mkdir shard");
                        let mut store = engine.open(&directory);
                        write_kv(&mut *store, per_thread, batch_size, seed);
                    });
                }
            });
        }
    }

    let elapsed = start.elapsed();
    let _ = std::fs::remove_dir_all(&base);

    let mut record = Record::new(COMPONENT, "concurrent_write", engine.name());
    record.params = json!({ "architecture": architecture.label(), "threads": threads, "writes_per_thread": per_thread, "batch_size": batch_size, "sync": true });
    record.throughput = Some(Throughput::new(total_ops, total_ops * 64, elapsed));
    record.extra = json!({ "aggregate_across_threads": true });
    record
}

/// Write `count` position-keyed leaves into a shared writer under bucket `bucket`.
fn write_bucket(
    writer: &mut dyn crate::engines::Writer,
    bucket: u64,
    count: usize,
    batch_size: usize,
    seed: u64,
) {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let mut position = 0u64;
    while (position as usize) < count {
        let mut batch = Vec::with_capacity(batch_size);
        for _ in 0..batch_size {
            if (position as usize) >= count {
                break;
            }
            batch.push((shared_key(bucket, position), value_of(&mut rng, 48)));
            position += 1;
        }
        writer.commit_batch(&batch, true);
    }
}

/// Write `count` position-keyed leaves into a per-bucket store (sharded).
fn write_kv(store: &mut dyn crate::engines::KvStore, count: usize, batch_size: usize, seed: u64) {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let mut position = 0u64;
    while (position as usize) < count {
        let mut batch = Vec::with_capacity(batch_size);
        for _ in 0..batch_size {
            if (position as usize) >= count {
                break;
            }
            batch.push((position_key(position), value_of(&mut rng, 48)));
            position += 1;
        }
        store.commit_batch(&batch, true);
    }
    store.flush();
}
