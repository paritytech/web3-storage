// SPDX-License-Identifier: Apache-2.0

//! Operation metrics shared across stress-test scenarios.
//!
//! Scenarios record one [`OpOutcome`] per individual operation (an upload
//! today; a read or delete later) and aggregate them with [`summarize`] into an
//! [`OpSummary`], which prints a uniform summary and is returned so callers can
//! consume the numbers programmatically. The aggregation is operation-agnostic:
//! adding a `read`/`delete` scenario means producing `Vec<OpOutcome>` and
//! tagging the summary via an [`Operation`] implementor — no new metrics code.

use std::fmt;
use std::time::Duration;

use clap::ValueEnum;
use serde::Serialize;

/// How to render the collected metrics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    Text,
    Json,
}

/// Display labels for one kind of operation. Captured into [`OpSummary`] by
/// [`summarize`] so a summary formats itself without knowing the concrete op.
#[derive(Debug, Clone, Copy)]
pub struct OpLabels {
    /// Lowercase verb, e.g. `"upload"`.
    pub verb: &'static str,
    /// Plural noun for counts/rates, e.g. `"uploads"`.
    pub noun_plural: &'static str,
    /// Past-tense verb for the data line, e.g. `"uploaded"`.
    pub past_tense: &'static str,
}

/// One kind of operation a stress-test scenario exercises. Implement it on a
/// marker type in the scenario's own module (e.g. `Upload` in the `stress_test`
/// command): the labels live with the operation, so there is no central list to
/// extend and nothing to keep in sync.
pub trait Operation {
    /// Display labels for this operation.
    fn labels(&self) -> OpLabels;
}

/// Outcome of a single operation attempt. A stress run records every outcome
/// (success or failure) rather than aborting, so failures surface in the
/// aggregate instead of stopping the run.
#[derive(Debug, Clone)]
pub struct OpOutcome {
    /// Whether the operation succeeded.
    pub ok: bool,
    /// Bytes transferred by this operation (uploaded/read; typically 0 for a
    /// delete).
    pub bytes: usize,
    /// Elapsed time the operation took.
    pub elapsed: Duration,
    /// Error message if the operation failed.
    pub err: Option<String>,
}

impl OpOutcome {
    /// Record a successful operation that transferred `bytes` in `elapsed`.
    pub fn success(bytes: usize, elapsed: Duration) -> Self {
        Self {
            ok: true,
            bytes,
            elapsed,
            err: None,
        }
    }

    /// Record a failed operation; `bytes` is what it would have transferred.
    pub fn failure(bytes: usize, elapsed: Duration, err: String) -> Self {
        Self {
            ok: false,
            bytes,
            elapsed,
            err: Some(err),
        }
    }
}

/// Number of sample error messages retained for the summary.
const MAX_SAMPLE_ERRORS: usize = 5;

/// Aggregated metrics over every [`OpOutcome`] of a single scenario run.
/// Returned from a scenario so the numbers (counts, bytes, timings) are
/// available to callers in addition to the printed summary.
#[derive(Debug, Clone)]
pub struct OpSummary {
    /// Display labels for the operation these metrics describe.
    pub labels: OpLabels,
    /// Total operations attempted.
    pub total: usize,
    /// Operations that succeeded.
    pub ok: usize,
    /// Operations that failed.
    pub failed: usize,
    /// Bytes transferred across the successful operations.
    pub bytes_ok: usize,
    /// Total elapsed (wall-clock) duration of the whole run — real time from
    /// first to last operation, not the sum of per-operation latencies.
    pub elapsed: Duration,
    /// Minimum latency over successful operations.
    pub lat_min: Duration,
    /// Mean latency over successful operations.
    pub lat_avg: Duration,
    /// Maximum latency over successful operations.
    pub lat_max: Duration,
    /// 50th-percentile (median) latency over successful operations.
    pub lat_p50: Duration,
    /// 95th-percentile latency over successful operations.
    pub lat_p95: Duration,
    /// 99th-percentile latency over successful operations.
    pub lat_p99: Duration,
    /// Up to [`MAX_SAMPLE_ERRORS`] sample messages from failed operations.
    pub sample_errors: Vec<String>,
}

impl OpSummary {
    /// Successful-byte throughput in bytes per second over the elapsed time.
    pub fn bytes_per_sec(&self) -> f64 {
        let secs = self.elapsed.as_secs_f64();
        if secs > 0.0 {
            self.bytes_ok as f64 / secs
        } else {
            0.0
        }
    }

    /// Successful-operation rate per second over the elapsed time.
    pub fn ops_per_sec(&self) -> f64 {
        let secs = self.elapsed.as_secs_f64();
        if secs > 0.0 {
            self.ok as f64 / secs
        } else {
            0.0
        }
    }
}

/// Nearest-rank percentile (`p` in `[0, 100]`) over an already-sorted slice of
/// durations. `Duration::ZERO` on an empty slice.
fn percentile(sorted: &[Duration], p: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    let idx = rank.saturating_sub(1).min(sorted.len() - 1);
    sorted[idx]
}

/// Aggregate per-operation outcomes against the elapsed duration of the run.
pub fn summarize(
    operation: impl Operation,
    outcomes: &[OpOutcome],
    elapsed: Duration,
) -> OpSummary {
    let total = outcomes.len();
    let ok = outcomes.iter().filter(|o| o.ok).count();
    let bytes_ok = outcomes.iter().filter(|o| o.ok).map(|o| o.bytes).sum();

    let mut ok_latencies: Vec<Duration> = outcomes
        .iter()
        .filter(|o| o.ok)
        .map(|o| o.elapsed)
        .collect();
    ok_latencies.sort_unstable();

    let lat_min = ok_latencies.first().copied().unwrap_or(Duration::ZERO);
    let lat_max = ok_latencies.last().copied().unwrap_or(Duration::ZERO);
    let lat_sum: Duration = ok_latencies.iter().sum();
    let lat_avg = lat_sum.checked_div(ok as u32).unwrap_or(Duration::ZERO);
    let lat_p50 = percentile(&ok_latencies, 50.0);
    let lat_p95 = percentile(&ok_latencies, 95.0);
    let lat_p99 = percentile(&ok_latencies, 99.0);

    let sample_errors = outcomes
        .iter()
        .filter_map(|o| o.err.clone())
        .take(MAX_SAMPLE_ERRORS)
        .collect();

    OpSummary {
        labels: operation.labels(),
        total,
        ok,
        failed: total - ok,
        bytes_ok,
        elapsed,
        lat_min,
        lat_avg,
        lat_max,
        lat_p50,
        lat_p95,
        lat_p99,
        sample_errors,
    }
}

/// JSON-friendly projection of [`OpSummary`]: operation as a lowercase string,
/// durations as fractional seconds, and the derived throughput rates inlined.
/// Decouples the on-disk/wire shape from the in-memory struct.
#[derive(Debug, Serialize)]
struct OpSummaryJson<'a> {
    operation: &'a str,
    total: usize,
    ok: usize,
    failed: usize,
    bytes_ok: usize,
    elapsed_secs: f64,
    throughput_bytes_per_sec: f64,
    throughput_ops_per_sec: f64,
    latency_min_secs: f64,
    latency_avg_secs: f64,
    latency_max_secs: f64,
    latency_p50_secs: f64,
    latency_p95_secs: f64,
    latency_p99_secs: f64,
    sample_errors: &'a [String],
}

impl OpSummary {
    fn as_json(&self) -> OpSummaryJson<'_> {
        OpSummaryJson {
            operation: self.labels.verb,
            total: self.total,
            ok: self.ok,
            failed: self.failed,
            bytes_ok: self.bytes_ok,
            elapsed_secs: self.elapsed.as_secs_f64(),
            throughput_bytes_per_sec: self.bytes_per_sec(),
            throughput_ops_per_sec: self.ops_per_sec(),
            latency_min_secs: self.lat_min.as_secs_f64(),
            latency_avg_secs: self.lat_avg.as_secs_f64(),
            latency_max_secs: self.lat_max.as_secs_f64(),
            latency_p50_secs: self.lat_p50.as_secs_f64(),
            latency_p95_secs: self.lat_p95.as_secs_f64(),
            latency_p99_secs: self.lat_p99.as_secs_f64(),
            sample_errors: &self.sample_errors,
        }
    }
}

/// Print a human-readable text summary for each scenario's metrics. The text
/// view of the collected results, kept in the CLI front-end so scenarios just
/// compute and return.
pub fn print_text(results: &[OpSummary]) {
    for m in results {
        print!("{m}");
    }
}

/// Render all scenario metrics as a pretty-printed JSON array (the `--output
/// json` view).
pub fn to_json(results: &[OpSummary]) -> serde_json::Result<String> {
    let view: Vec<OpSummaryJson> = results.iter().map(OpSummary::as_json).collect();
    serde_json::to_string_pretty(&view)
}

impl fmt::Display for OpSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let secs = self.elapsed.as_secs_f64();
        let mib = self.bytes_ok as f64 / (1024.0 * 1024.0);
        let mib_s = self.bytes_per_sec() / (1024.0 * 1024.0);
        let ops = self.ops_per_sec();
        writeln!(
            f,
            "{} results: {} total, {} ok, {} failed",
            self.labels.verb, self.total, self.ok, self.failed
        )?;
        writeln!(
            f,
            "  data:       {:.2} MiB {} in {:.3}s",
            mib, self.labels.past_tense, secs
        )?;
        writeln!(
            f,
            "  throughput: {mib_s:.2} MiB/s, {ops:.1} {}/s",
            self.labels.noun_plural
        )?;
        writeln!(
            f,
            "  latency:    min {:.3}s / avg {:.3}s / max {:.3}s",
            self.lat_min.as_secs_f64(),
            self.lat_avg.as_secs_f64(),
            self.lat_max.as_secs_f64(),
        )?;
        writeln!(
            f,
            "  percentile: p50 {:.3}s / p95 {:.3}s / p99 {:.3}s",
            self.lat_p50.as_secs_f64(),
            self.lat_p95.as_secs_f64(),
            self.lat_p99.as_secs_f64(),
        )?;
        if !self.sample_errors.is_empty() {
            writeln!(f, "  sample errors:")?;
            for e in &self.sample_errors {
                writeln!(f, "    - {e}")?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal operation fixture so the generic machinery is testable without
    /// depending on any concrete scenario's operation type.
    struct TestOp;
    impl Operation for TestOp {
        fn labels(&self) -> OpLabels {
            OpLabels {
                verb: "test",
                noun_plural: "tests",
                past_tense: "tested",
            }
        }
    }

    #[test]
    fn summarize_counts_and_bytes() {
        let outcomes = vec![
            OpOutcome::success(100, Duration::from_millis(10)),
            OpOutcome::success(100, Duration::from_millis(30)),
            OpOutcome::failure(100, Duration::from_millis(5), "boom".into()),
        ];
        let m = summarize(TestOp, &outcomes, Duration::from_secs(1));
        assert_eq!(m.labels.verb, "test");
        assert_eq!(m.total, 3);
        assert_eq!(m.ok, 2);
        assert_eq!(m.failed, 1);
        assert_eq!(m.bytes_ok, 200);
        assert_eq!(m.lat_min, Duration::from_millis(10));
        assert_eq!(m.lat_max, Duration::from_millis(30));
        assert_eq!(m.lat_avg, Duration::from_millis(20));
        assert_eq!(m.lat_p50, Duration::from_millis(10));
        assert_eq!(m.lat_p95, Duration::from_millis(30));
        assert_eq!(m.lat_p99, Duration::from_millis(30));
        assert_eq!(m.ops_per_sec(), 2.0);
        assert_eq!(m.sample_errors, vec!["boom".to_string()]);
    }

    #[test]
    fn summarize_all_failed_has_zero_latency() {
        let outcomes = vec![OpOutcome::failure(
            50,
            Duration::from_millis(5),
            "nope".into(),
        )];
        let m = summarize(TestOp, &outcomes, Duration::from_secs(1));
        assert_eq!(m.ok, 0);
        assert_eq!(m.bytes_ok, 0);
        assert_eq!(m.lat_min, Duration::ZERO);
        assert_eq!(m.lat_avg, Duration::ZERO);
        assert_eq!(m.lat_max, Duration::ZERO);
        assert_eq!(m.lat_p50, Duration::ZERO);
        assert_eq!(m.lat_p95, Duration::ZERO);
        assert_eq!(m.lat_p99, Duration::ZERO);
    }

    #[test]
    fn percentile_uses_nearest_rank() {
        let durations: Vec<Duration> = (1..=100).map(Duration::from_millis).collect();
        assert_eq!(percentile(&durations, 50.0), Duration::from_millis(50));
        assert_eq!(percentile(&durations, 95.0), Duration::from_millis(95));
        assert_eq!(percentile(&durations, 99.0), Duration::from_millis(99));
    }

    #[test]
    fn percentile_of_empty_is_zero() {
        assert_eq!(percentile(&[], 50.0), Duration::ZERO);
    }

    #[test]
    fn to_json_projects_expected_fields() {
        let outcomes = vec![
            OpOutcome::success(1024, Duration::from_millis(10)),
            OpOutcome::failure(1024, Duration::from_millis(5), "boom".into()),
        ];
        let m = summarize(TestOp, &outcomes, Duration::from_secs(2));
        let json = to_json(&[m]).expect("serializes");
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid json");
        let entry = &v[0];
        assert_eq!(entry["operation"], "test");
        assert_eq!(entry["total"], 2);
        assert_eq!(entry["ok"], 1);
        assert_eq!(entry["failed"], 1);
        assert_eq!(entry["bytes_ok"], 1024);
        assert_eq!(entry["elapsed_secs"], 2.0);
        assert_eq!(entry["throughput_ops_per_sec"], 0.5);
        assert_eq!(entry["latency_avg_secs"], 0.01);
        assert_eq!(entry["latency_p50_secs"], 0.01);
        assert_eq!(entry["latency_p95_secs"], 0.01);
        assert_eq!(entry["latency_p99_secs"], 0.01);
        assert_eq!(entry["sample_errors"][0], "boom");
    }
}
