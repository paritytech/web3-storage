//! Measurement utilities: latency percentiles, process RSS, open file
//! descriptors, and on-disk size. The process-level probes read Linux `/proc`
//! and return `None` on other platforms so the harness still runs (without
//! those columns) elsewhere.

use serde::Serialize;
use std::path::Path;
use std::time::Duration;

/// Latency distribution over a set of timed operations, in microseconds.
#[derive(Debug, Clone, Serialize, Default)]
pub struct LatencyStats {
    pub count: u64,
    pub mean_us: f64,
    pub p50_us: f64,
    pub p90_us: f64,
    pub p99_us: f64,
    pub max_us: f64,
}

impl LatencyStats {
    /// Compute percentiles from a set of per-operation durations.
    pub fn from_durations(mut samples: Vec<Duration>) -> Self {
        if samples.is_empty() {
            return Self::default();
        }
        samples.sort_unstable();
        let count = samples.len();
        let micros = |duration: &Duration| duration.as_secs_f64() * 1_000_000.0;
        let percentile = |percent: f64| {
            // Nearest-rank percentile.
            let rank = ((percent / 100.0) * count as f64).ceil() as usize;
            let index = rank.saturating_sub(1).min(count - 1);
            micros(&samples[index])
        };
        let sum: f64 = samples.iter().map(micros).sum();
        Self {
            count: count as u64,
            mean_us: sum / count as f64,
            p50_us: percentile(50.0),
            p90_us: percentile(90.0),
            p99_us: percentile(99.0),
            max_us: micros(&samples[count - 1]),
        }
    }
}

/// Throughput derived from a count of operations over a wall-clock window.
#[derive(Debug, Clone, Serialize, Default)]
pub struct Throughput {
    pub ops: u64,
    pub bytes: u64,
    pub elapsed_s: f64,
    pub ops_per_s: f64,
    pub mib_per_s: f64,
}

impl Throughput {
    pub fn new(ops: u64, bytes: u64, elapsed: Duration) -> Self {
        let elapsed_s = elapsed.as_secs_f64().max(f64::MIN_POSITIVE);
        Self {
            ops,
            bytes,
            elapsed_s,
            ops_per_s: ops as f64 / elapsed_s,
            mib_per_s: (bytes as f64 / (1024.0 * 1024.0)) / elapsed_s,
        }
    }
}

/// Resident set size (how much RAM the process is using) of this process in bytes,
/// via `/proc/self/statm`.
pub fn process_rss_bytes() -> Option<u64> {
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let resident_pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    let page_size = 4096u64; // Standard on the benchmark host; documented in the report.
    Some(resident_pages * page_size)
}

/// Number of open file descriptors held by this process, via `/proc/self/fd`.
pub fn open_fd_count() -> Option<u64> {
    let entries = std::fs::read_dir("/proc/self/fd").ok()?;
    Some(entries.count() as u64)
}

/// Total size in bytes of all files under `path` (recursive).
pub fn directory_size_bytes(path: &Path) -> u64 {
    fn walk(path: &Path, total: &mut u64) {
        let Ok(metadata) = std::fs::symlink_metadata(path) else {
            return;
        };
        if metadata.is_file() {
            *total += metadata.len();
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
