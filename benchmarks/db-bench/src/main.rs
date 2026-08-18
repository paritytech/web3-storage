//! Database engine benchmark harness.
//!
//! Compares candidate engines on the Storage Provider's workloads across both
//! architectures under evaluation: sharded (one DB per bucket) vs shared (a
//! single DB for all buckets) — Sled/SQLite/redb/RocksDB, plus ParityDB in the
//! shared model.
//!
//! Results are emitted as JSON (one record per engine × scenario) for the
//! reports under `docs/design/database-evaluation/`.
//!
//! Examples:
//!   cargo run -p db-bench --release -- --output results.json
//!   cargo run -p db-bench --release -- --engine sqlite --quick

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
}

/// Sharded (per-bucket-file) candidates — engines viable as many small instances.
fn sharded_engines() -> Vec<Engine> {
    vec![Engine::Sled, Engine::Sqlite, Engine::Redb, Engine::Rocksdb]
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

    std::fs::create_dir_all(&cli.work_directory).expect("create work directory");
    let context = Context {
        work_directory: cli.work_directory.clone(),
        seed: cli.seed,
        scale,
    };

    let mut all_records = Vec::new();

    {
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
