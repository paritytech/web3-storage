//! Database engine benchmark harness.
//!
//! Compares candidate engines on the Storage Provider's workloads across both
//! architectures under evaluation: sharded (one DB per bucket) vs shared (a
//! single DB for all buckets).
//!
//! Sharded candidates are the engines that are cheap to instantiate many times
//! over — Sled, SQLite, redb, RocksDB, plus the mmap'd single-file B+trees
//! (LMDB, libmdbx, jammdb) that spawn no background threads at all. The shared
//! model adds ParityDB, whose per-instance overhead rules it out of sharding.
//!
//! Results are emitted as JSON (one record per engine × scenario) for the
//! reports under `docs/design/database-evaluation/`.
//!
//! This crate is its own workspace, excluded from the repository's root
//! workspace, so cargo must be pointed at its manifest (or run from this
//! directory).
//!
//! Examples, from the repository root:
//!   cargo run --release --manifest-path benchmarks/db-bench/Cargo.toml -- --output results.json
//!   cargo run --release --manifest-path benchmarks/db-bench/Cargo.toml -- --engine sqlite --quick

mod engines;
mod metrics;
mod workloads;

use clap::Parser;
use engines::Engine;
use std::path::PathBuf;
use workloads::Context;

#[derive(Parser, Debug)]
#[command(about = "Database engine benchmark harness")]
struct Cli {
    /// Restrict to a single engine (otherwise the full candidate set).
    #[arg(long, value_enum)]
    engine: Option<Engine>,

    /// Scratch directory for benchmark databases.
    #[arg(long, default_value = "/tmp/db-bench")]
    work_directory: PathBuf,

    /// Write JSON results here (stdout if omitted).
    #[arg(long)]
    output: Option<PathBuf>,

    /// RNG seed for reproducibility.
    #[arg(long, default_value_t = 0xC0FFEE)]
    seed: u64,

    /// Shrink every scenario for a fast smoke run.
    #[arg(long)]
    quick: bool,

    /// Explicit scale factor (overrides --quick); 1.0 = full sizes.
    #[arg(long)]
    scale: Option<f64>,

    /// Per-environment `map_size` for LMDB, in MiB.
    ///
    /// LMDB alone requires this ceiling up front and maps it all at open, so
    /// `map_size × open_instances` is reserved address space. Raising it is how
    /// you find where a database-per-bucket design stops fitting.
    #[arg(long, default_value_t = 1024)]
    lmdb_map_size_mib: usize,

    /// Run only the dedup-lookup probe, which measures absent-key latency
    /// against write-ahead-log size instead of the full scenario suite.
    #[arg(long)]
    dedup_probe: bool,

    /// SQLite page cache in KiB; 0 leaves the 2 MiB default.
    #[arg(long, default_value_t = 0)]
    sqlite_cache_size_kib: usize,

    /// SQLite `mmap_size` in MiB; 0 leaves mmap disabled (the default).
    #[arg(long, default_value_t = 0)]
    sqlite_mmap_size_mib: usize,

    /// Give SQLite's key index its own B-tree instead of storing payloads in it.
    #[arg(long)]
    sqlite_split_index: bool,

    /// SQLite `page_size` in bytes; 0 leaves the 4096-byte default.
    ///
    /// A 256 KiB chunk spans ~64 default pages and is read one page at a time,
    /// so this is the knob that decides SQLite's content-store read latency —
    /// at the cost of a larger floor for an empty bucket.
    #[arg(long, default_value_t = 0)]
    sqlite_page_size: usize,
}

/// Sharded (per-bucket-file) candidates — engines viable as many small instances.
fn sharded_engines() -> Vec<Engine> {
    vec![
        Engine::Sled,
        Engine::Sqlite,
        Engine::Redb,
        Engine::Rocksdb,
        Engine::Lmdb,
        Engine::Mdbx,
        Engine::Jammdb,
    ]
}
/// Shared (single-DB) candidates — per-instance overhead no longer matters, so
/// ParityDB joins as a fifth candidate.
fn shared_engines() -> Vec<Engine> {
    vec![
        Engine::Rocksdb,
        Engine::Sled,
        Engine::Sqlite,
        Engine::Redb,
        Engine::Paritydb,
    ]
}

/// Apply the optional `--engine` filter to a candidate list.
fn filtered(engines: Vec<Engine>, only: Option<Engine>) -> Vec<Engine> {
    match only {
        Some(selected) => engines
            .into_iter()
            .filter(|&candidate| candidate == selected)
            .collect(),
        None => engines,
    }
}

/// Stamp `architecture` (Sharded | Shared) into every record's params object.
fn tag_architecture(records: &mut [workloads::Record], architecture: &str) {
    for record in records {
        if let Some(object) = record.params.as_object_mut() {
            object.insert("architecture".into(), serde_json::json!(architecture));
        }
    }
}

fn main() {
    let cli = Cli::parse();
    let scale = cli.scale.unwrap_or(if cli.quick { 0.02 } else { 1.0 });

    engines::set_lmdb_map_size(cli.lmdb_map_size_mib * 1024 * 1024);
    engines::set_sqlite_page_size(cli.sqlite_page_size);
    engines::set_sqlite_cache_size_kib(cli.sqlite_cache_size_kib);
    engines::set_sqlite_mmap_size(cli.sqlite_mmap_size_mib * 1024 * 1024);
    engines::set_sqlite_split_index(cli.sqlite_split_index);

    std::fs::create_dir_all(&cli.work_directory).expect("create work directory");
    let context = Context {
        work_directory: cli.work_directory.clone(),
        seed: cli.seed,
        scale,
    };

    let mut all_records = Vec::new();

    if cli.dedup_probe {
        for engine in filtered(sharded_engines(), cli.engine) {
            eprintln!("[dedup-probe] running {} ...", engine.name());
            all_records.extend(workloads::storage::run_dedup_probe(engine, &context));
        }
    } else {
        use workloads::storage_shared::Architecture;

        // Sharded architecture: one DB file per bucket.
        for engine in filtered(sharded_engines(), cli.engine) {
            eprintln!("[storage/sharded] running {} ...", engine.name());
            let mut records = workloads::storage::run_all(engine, &context);
            tag_architecture(&mut records, "sharded");
            all_records.extend(records);
        }

        // Shared architecture: one DB holding all buckets.
        for engine in filtered(shared_engines(), cli.engine) {
            eprintln!("[storage/shared] running {} ...", engine.name());
            all_records.extend(workloads::storage_shared::run_all(engine, &context));
        }

        // Concurrent multi-bucket writes — the decisive cross-architecture test.
        for engine in filtered(sharded_engines(), cli.engine) {
            eprintln!("[storage/concurrent sharded] running {} ...", engine.name());
            all_records.push(workloads::storage_shared::concurrent_write(
                engine,
                &context,
                Architecture::Sharded,
            ));
        }
        for engine in filtered(shared_engines(), cli.engine) {
            eprintln!("[storage/concurrent shared] running {} ...", engine.name());
            all_records.push(workloads::storage_shared::concurrent_write(
                engine,
                &context,
                Architecture::Shared,
            ));
        }
    }

    let output = serde_json::json!({
        "meta": {
            "seed": cli.seed,
            "scale": scale,
            "host": std::env::var("HOSTNAME").unwrap_or_default(),
            "page_size_assumed": 4096,
            "lmdb_map_size_bytes": engines::lmdb_map_size(),
            "sqlite_page_size": engines::sqlite_page_size(),
            "sqlite_cache_size_kib": engines::sqlite_cache_size_kib(),
            "sqlite_mmap_size_bytes": engines::sqlite_mmap_size(),
            "sqlite_split_index": engines::sqlite_split_index(),
        },
        "records": all_records,
    });
    let json = serde_json::to_string_pretty(&output).expect("serialize");

    match &cli.output {
        Some(path) => {
            std::fs::write(path, json).expect("write results");
            eprintln!("wrote results to {}", path.display());
        }
        None => println!("{json}"),
    }

    // Best-effort cleanup of scratch DBs.
    let _ = std::fs::remove_dir_all(&cli.work_directory);
}
