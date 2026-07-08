//! Database engine benchmark harness.
//!
//! Compares candidate engines on the two component workloads:
//!   * Storage Provider: sharded (per-bucket DB) vs shared (single DB)
//!     architectures — Sled/SQLite/RocksDB, plus ParityDB in the shared model
//!   * Blockchain Node (state trie): RocksDB vs ParityDB
//!
//! Results are emitted as JSON (one record per engine × scenario) for the
//! reports under `docs/design/database-evaluation/`.
//!
//! Examples:
//!   cargo run -p db-bench --release -- --component storage --output results.json
//!   cargo run -p db-bench --release -- --component all --quick

mod engines;
mod metrics;
mod workloads;

use clap::{Parser, ValueEnum};
use engines::Engine;
use std::path::PathBuf;
use workloads::Context;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Component {
    /// Storage Provider workloads, both sharded and shared architectures.
    Storage,
    /// Blockchain Node state-trie workloads (rocksdb, paritydb).
    Blockchain,
    /// Both components.
    All,
}

#[derive(Parser, Debug)]
#[command(about = "Database engine benchmark harness")]
struct Cli {
    /// Which component's workloads to run.
    #[arg(long, value_enum, default_value_t = Component::All)]
    component: Component,

    /// Restrict to a single engine (otherwise the component's full candidate set).
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
fn storage_sharded_engines() -> Vec<Engine> {
    vec![Engine::Sled, Engine::Sqlite, Engine::Rocksdb]
}
/// Shared (single-DB) candidates — per-instance overhead no longer matters, so
/// ParityDB joins as a fourth candidate.
fn storage_shared_engines() -> Vec<Engine> {
    vec![
        Engine::Rocksdb,
        Engine::Sled,
        Engine::Sqlite,
        Engine::Paritydb,
    ]
}
fn blockchain_engines() -> Vec<Engine> {
    vec![Engine::Rocksdb, Engine::Paritydb]
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

    let run_storage = matches!(cli.component, Component::Storage | Component::All);
    let run_blockchain = matches!(cli.component, Component::Blockchain | Component::All);

    if run_storage {
        use workloads::storage_shared::Architecture;

        // Sharded architecture: one DB file per bucket.
        for engine in filtered(storage_sharded_engines(), cli.engine) {
            eprintln!("[storage/sharded] running {} ...", engine.name());
            let mut records = workloads::storage::run_all(engine, &context);
            tag_architecture(&mut records, "sharded");
            all_records.extend(records);
        }

        // Shared architecture: one DB holding all buckets.
        for engine in filtered(storage_shared_engines(), cli.engine) {
            eprintln!("[storage/shared] running {} ...", engine.name());
            all_records.extend(workloads::storage_shared::run_all(engine, &context));
        }

        // Concurrent multi-bucket writes — the decisive cross-architecture test.
        for engine in filtered(storage_sharded_engines(), cli.engine) {
            eprintln!("[storage/concurrent sharded] running {} ...", engine.name());
            all_records.push(workloads::storage_shared::concurrent_write(
                engine,
                &context,
                Architecture::Sharded,
            ));
        }
        for engine in filtered(storage_shared_engines(), cli.engine) {
            eprintln!("[storage/concurrent shared] running {} ...", engine.name());
            all_records.push(workloads::storage_shared::concurrent_write(
                engine,
                &context,
                Architecture::Shared,
            ));
        }
    }

    if run_blockchain {
        for engine in filtered(blockchain_engines(), cli.engine) {
            eprintln!("[blockchain] running {} ...", engine.name());
            all_records.extend(workloads::state_trie::run_all(engine, &context));
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
