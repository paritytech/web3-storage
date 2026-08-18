//! Benchmark scenarios and shared helpers.

pub mod storage;
pub mod storage_shared;

use crate::metrics::{LatencyStats, Throughput};
use rand::RngCore;
use rand_chacha::rand_core::SeedableRng;
use rand_chacha::ChaCha8Rng;
use serde::Serialize;
use std::path::{Path, PathBuf};

/// One measured scenario result. Fields not relevant to a scenario stay `None`.
#[derive(Debug, Clone, Serialize)]
pub struct Record {
    /// Always `"storage_provider"`.
    pub component: String,
    pub scenario: String,
    pub engine: String,
    pub params: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency: Option<LatencyStats>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub throughput: Option<Throughput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rss_delta_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fd_delta: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disk_bytes: Option<u64>,
    /// Free-form scenario-specific measurements.
    pub extra: serde_json::Value,
}

impl Record {
    pub fn new(component: &str, scenario: &str, engine: &str) -> Self {
        Self {
            component: component.to_string(),
            scenario: scenario.to_string(),
            engine: engine.to_string(),
            params: serde_json::Value::Null,
            latency: None,
            throughput: None,
            rss_delta_bytes: None,
            fd_delta: None,
            disk_bytes: None,
            extra: serde_json::Value::Null,
        }
    }
}

/// Shared run context: where scratch DBs live, and the RNG seed.
pub struct Context {
    pub work_directory: PathBuf,
    pub seed: u64,
    /// Scale knob (1 = full sizes). Lets `--quick` shrink every scenario.
    pub scale: f64,
}

impl Context {
    /// A fresh, empty scratch directory for one store instance.
    pub fn fresh_directory(&self, tag: &str) -> PathBuf {
        let path = self.work_directory.join(tag);
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create scratch directory");
        path
    }

    /// Apply the scale knob to a size, with a floor of 1.
    ///
    /// Allows running benchmarks with smaller workloads.
    pub fn scaled(&self, count: usize) -> usize {
        ((count as f64 * self.scale) as usize).max(1)
    }
}

/// Remove a directory tree, returning how long the removal took.
pub fn remove_tree_timed(path: &Path) -> std::time::Duration {
    let started = std::time::Instant::now();
    let _ = std::fs::remove_dir_all(path);
    started.elapsed()
}

/// 8-byte big-endian position MMR key in a split scenario.
pub fn position_key(position: u64) -> Vec<u8> {
    position.to_be_bytes().to_vec()
}

/// 16-byte key for the shared-DB architecture: `bucket_id || position`, both
/// big-endian so a bucket's entries sort contiguously.
pub fn shared_key(bucket: u64, position: u64) -> Vec<u8> {
    let mut key = Vec::with_capacity(16);
    key.extend_from_slice(&bucket.to_be_bytes());
    key.extend_from_slice(&position.to_be_bytes());
    key
}

/// A value of `size` bytes filled deterministically.
pub fn value_of(rng: &mut ChaCha8Rng, size: usize) -> Vec<u8> {
    let mut value = vec![0u8; size];
    rng.fill_bytes(&mut value);
    value
}
