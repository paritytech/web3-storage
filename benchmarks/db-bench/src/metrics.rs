//! Measurement utilities: latency percentiles, process RSS, virtual address
//! space, thread count, open file descriptors, and on-disk size. The
//! process-level probes read Linux `/proc` and return `None` on other platforms
//! so the harness still runs (without those columns) elsewhere.
//!
//! On-disk size is reported two ways, and the distinction is load-bearing:
//! [`directory_size_bytes`] sums *apparent* file lengths while
//! [`directory_allocated_bytes`] sums *allocated blocks*. They diverge for
//! engines that preallocate a sparse file — LMDB maps its whole `map_size` up
//! front, so its apparent size is the ceiling it was given and tells you nothing
//! about the bytes it actually occupies.

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

/// Virtual address space of this process in bytes, via `/proc/self/statm`.
///
/// The metric that bounds mmap-based engines: LMDB reserves its entire
/// `map_size` per environment at open, so with one environment per bucket the
/// address space, not the memory, is what runs out first.
pub fn process_vsize_bytes() -> Option<u64> {
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let size_pages: u64 = statm.split_whitespace().next()?.parse().ok()?;
    let page_size = 4096u64; // Standard on the benchmark host; documented in the report.
    Some(size_pages * page_size)
}

/// Number of OS threads in this process, via `/proc/self/task`.
///
/// The per-instance cost the earlier reports never measured. Engines with
/// background compaction or flusher threads multiply this by every open
/// instance; the mmap'd B+trees (LMDB, mdbx, jammdb) and SQLite add none.
pub fn process_thread_count() -> Option<u64> {
    let entries = std::fs::read_dir("/proc/self/task").ok()?;
    Some(entries.count() as u64)
}

/// Number of open file descriptors held by this process, via `/proc/self/fd`.
pub fn open_fd_count() -> Option<u64> {
    let entries = std::fs::read_dir("/proc/self/fd").ok()?;
    Some(entries.count() as u64)
}

/// Bytes held in write-ahead / log files under `path` — the un-checkpointed
/// write set an engine has accumulated but not yet folded into its main file.
///
/// Matches SQLite's `-wal`, RocksDB's `.log`, and the generic `.wal` suffix.
/// Engines with no such file (LMDB, jammdb) report 0, which is the honest
/// answer: they have no write set to consult on a read.
pub fn write_ahead_bytes(path: &Path) -> u64 {
    fn walk(path: &Path, total: &mut u64) {
        let Ok(metadata) = std::fs::symlink_metadata(path) else {
            return;
        };
        if metadata.is_file() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.ends_with("-wal") || name.ends_with(".wal") || name.ends_with(".log") {
                *total += metadata.len();
            }
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

/// Blocks actually allocated to all files under `path`, in bytes (recursive).
///
/// Unlike [`directory_size_bytes`] this is sparse-aware: a file whose length is
/// 1 GiB but which has only 40 KiB of blocks written counts as 40 KiB. Use this
/// for any real space comparison.
#[cfg(unix)]
pub fn directory_allocated_bytes(path: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt;

    fn walk(path: &Path, total: &mut u64) {
        let Ok(metadata) = std::fs::symlink_metadata(path) else {
            return;
        };
        if metadata.is_file() {
            // `st_blocks` is always in 512-byte units, independent of block size.
            *total += metadata.blocks() * 512;
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

/// Non-Unix fallback: no `st_blocks`, so apparent size is the best available.
#[cfg(not(unix))]
pub fn directory_allocated_bytes(path: &Path) -> u64 {
    directory_size_bytes(path)
}

/// Total *apparent* size in bytes of all files under `path` (recursive).
///
/// Counts file lengths, so a sparse preallocated file counts at its full
/// declared length. See [`directory_allocated_bytes`] for real usage.
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
