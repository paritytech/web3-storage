// SPDX-License-Identifier: Apache-2.0

//! Support shared by the `soak` commands: run settings parsing, polling, stop
//! handling, provider probes and the report.

use std::fmt;
use std::future::Future;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::Serialize;

use crate::common::BucketId;
use crate::metrics::{summarize, OpLabels, OpOutcome, OpSummary, Operation};

/// Parse `90s`, `30m` or `4h`: a positive integer and a unit.
pub fn parse_duration(raw: &str) -> Result<Duration, String> {
    let invalid = || format!("must look like 90s, 30m or 4h, got \"{raw}\"");
    let unit = raw.chars().last().ok_or_else(invalid)?;
    let unit_secs = match unit {
        's' => 1,
        'm' => 60,
        'h' => 3600,
        _ => return Err(invalid()),
    };
    let digits = &raw[..raw.len() - 1];
    if !digits.chars().all(|c| c.is_ascii_digit()) {
        return Err(invalid());
    }
    match digits.parse::<u64>() {
        Ok(n) if n > 0 => n
            .checked_mul(unit_secs)
            .map(Duration::from_secs)
            .ok_or_else(invalid),
        _ => Err(invalid()),
    }
}

const FINISH: u8 = 1;
const ABORT: u8 = 2;

/// Run-wide stop state. `finish` means no new work starts; `abort` means
/// running work stops at its next step.
#[derive(Clone, Default)]
pub struct StopSignal(Arc<AtomicU8>);

impl StopSignal {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn finish(&self) {
        self.0.fetch_max(FINISH, Ordering::SeqCst);
    }

    pub fn abort(&self) {
        self.0.store(ABORT, Ordering::SeqCst);
    }

    pub fn should_finish(&self) -> bool {
        self.0.load(Ordering::SeqCst) >= FINISH
    }

    pub fn is_aborted(&self) -> bool {
        self.0.load(Ordering::SeqCst) == ABORT
    }

    /// First Ctrl-C calls `finish`; a second exits the process at once.
    pub fn listen_for_ctrl_c(&self) {
        let stop = self.clone();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_err() {
                return;
            }
            eprintln!("Ctrl-C: finishing the current cycles (press again to exit now)");
            stop.finish();
            if tokio::signal::ctrl_c().await.is_ok() {
                std::process::exit(130);
            }
        });
    }
}

/// Call `probe` every `step` until it returns `Some`. Errors when `probe`
/// errors, after `timeout` (naming `what`), or when the run is aborted.
pub async fn poll_until<T, F, Fut>(
    what: &str,
    timeout: Duration,
    step: Duration,
    stop: &StopSignal,
    probe: F,
) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<Option<T>>>,
{
    let mut probe = probe;
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if stop.is_aborted() {
            bail!("aborted while waiting for {what}");
        }
        if let Some(value) = probe().await? {
            return Ok(value);
        }
        if tokio::time::Instant::now() >= deadline {
            bail!(
                "timed out after {} s waiting for {what}",
                timeout.as_secs_f64()
            );
        }
        tokio::time::sleep(step).await;
    }
}

/// Resident memory in KB of the process listening on a local provider URL's
/// port. `None` for a remote URL or when `lsof` or `ps` fail.
pub fn provider_rss_kb(provider_url: &str) -> Option<u64> {
    let port = local_port(provider_url)?;
    let listening = Command::new("lsof")
        .args(["-ti", &format!("tcp:{port}"), "-sTCP:LISTEN"])
        .output()
        .ok()?;
    let pid: u32 = String::from_utf8_lossy(&listening.stdout)
        .lines()
        .next()?
        .trim()
        .parse()
        .ok()?;
    let rss = Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    String::from_utf8_lossy(&rss.stdout)
        .trim()
        .parse()
        .ok()
        .filter(|kb| *kb > 0)
}

/// Port of a provider URL whose host is this machine.
fn local_port(provider_url: &str) -> Option<u16> {
    let authority = provider_url.split_once("://")?.1.split('/').next()?;
    let (host, port) = authority.rsplit_once(':')?;
    matches!(host, "127.0.0.1" | "localhost" | "[::1]")
        .then(|| port.parse().ok())
        .flatten()
}

/// Provider log lines of the garbage collector.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct GcLogCounts {
    /// Lines saying a reconcile was skipped.
    pub reconcile_skipped: u64,
    /// Lines logged at `WARN` or `ERROR`, skipped reconciles included.
    pub warn_or_error: u64,
}

/// Count the garbage collector's lines in `text`, ignoring colour codes.
pub fn count_gc_lines(text: &str) -> GcLogCounts {
    let mut counts = GcLogCounts::default();
    for line in strip_ansi(text).lines() {
        if !line.contains("provider_garbage_collector") {
            continue;
        }
        if line.contains("GC reconcile skipped") {
            counts.reconcile_skipped += 1;
        }
        if line.split_whitespace().any(|w| w == "WARN" || w == "ERROR") {
            counts.warn_or_error += 1;
        }
    }
    counts
}

/// `text` without ANSI colour sequences.
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            for end in chars.by_ref() {
                if end.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Reads the provider log lines appended since the last read.
pub struct LogTail {
    path: PathBuf,
    offset: u64,
}

impl LogTail {
    /// Start at the current end of the file.
    pub fn open(path: PathBuf) -> Result<Self> {
        let offset = std::fs::metadata(&path)
            .with_context(|| format!("failed to read provider log {}", path.display()))?
            .len();
        Ok(Self { path, offset })
    }

    /// Counts the lines appended since the last call. A shorter file is read from the start.
    pub fn read_new(&mut self) -> Result<GcLogCounts> {
        let mut file = std::fs::File::open(&self.path)
            .with_context(|| format!("failed to read provider log {}", self.path.display()))?;
        let size = file.metadata()?.len();
        if size < self.offset {
            self.offset = 0;
        }
        file.seek(SeekFrom::Start(self.offset))?;
        let mut appended = Vec::new();
        file.read_to_end(&mut appended)?;
        self.offset += appended.len() as u64;
        Ok(count_gc_lines(&String::from_utf8_lossy(&appended)))
    }
}

/// An invariant break and where it happened.
#[derive(Debug, Clone, Serialize)]
pub struct SoakFailure {
    pub message: String,
    pub epoch: u32,
    pub lane: Option<usize>,
    pub cycle: Option<u32>,
}

impl fmt::Display for SoakFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FAILED epoch {}", self.epoch)?;
        if let Some(lane) = self.lane {
            write!(f, " lane {lane}")?;
        }
        if let Some(cycle) = self.cycle {
            write!(f, " cycle {cycle}")?;
        }
        write!(f, ": {}", self.message)
    }
}

/// One prune cycle of one lane.
#[derive(Debug, Clone, Serialize)]
pub struct CycleRecord {
    pub lane: usize,
    pub epoch: u32,
    pub cycle: u32,
    pub bucket_id: BucketId,
    pub uploaded_bytes: u64,
    pub erase_latency_secs: f64,
    pub used_bytes_after: u64,
}

/// The drain check after one epoch.
#[derive(Debug, Clone, Serialize)]
pub struct BarrierRecord {
    pub epoch: u32,
    pub drain_secs: f64,
    pub provider_rss_kb: Option<u64>,
    pub gc_log: Option<GcLogCounts>,
}

/// GC erasure of pruned bytes, from the prune checkpoint to the usage drop.
pub struct Erase;

impl Operation for Erase {
    fn labels(&self) -> OpLabels {
        OpLabels {
            verb: "erase",
            noun_plural: "erasures",
            past_tense: "erased",
        }
    }
}

/// Everything a soak run recorded.
#[derive(Debug, Clone)]
pub struct SoakReport {
    pub service: &'static str,
    pub cycles: Vec<CycleRecord>,
    pub barriers: Vec<BarrierRecord>,
    pub failure: Option<SoakFailure>,
    pub elapsed: Duration,
}

impl SoakReport {
    /// Erase latency of the cycles of one epoch.
    pub fn epoch_summary(&self, epoch: u32) -> OpSummary {
        let outcomes: Vec<OpOutcome> = self
            .cycles
            .iter()
            .filter(|c| c.epoch == epoch)
            .map(|c| OpOutcome::success(0, Duration::from_secs_f64(c.erase_latency_secs)))
            .collect();
        summarize(Erase, &outcomes, self.elapsed)
    }

    pub fn to_json(&self) -> serde_json::Result<String> {
        #[derive(Serialize)]
        struct View<'a> {
            service: &'a str,
            elapsed_secs: f64,
            cycles: &'a [CycleRecord],
            barriers: &'a [BarrierRecord],
            failure: Option<&'a SoakFailure>,
        }
        serde_json::to_string_pretty(&View {
            service: self.service,
            elapsed_secs: self.elapsed.as_secs_f64(),
            cycles: &self.cycles,
            barriers: &self.barriers,
            failure: self.failure.as_ref(),
        })
    }
}

impl fmt::Display for SoakReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "{} soak: {:.1}s",
            self.service,
            self.elapsed.as_secs_f64()
        )?;
        for barrier in &self.barriers {
            let erase = self.epoch_summary(barrier.epoch);
            writeln!(
                f,
                "epoch {}: {} cycles, erase min {:.1}s p50 {:.1}s p95 {:.1}s max {:.1}s, drain {:.1}s, provider RSS {} KB, GC warnings {}",
                barrier.epoch,
                erase.total,
                erase.lat_min.as_secs_f64(),
                erase.lat_p50.as_secs_f64(),
                erase.lat_p95.as_secs_f64(),
                erase.lat_max.as_secs_f64(),
                barrier.drain_secs,
                barrier
                    .provider_rss_kb
                    .map_or("unavailable".to_string(), |kb| kb.to_string()),
                barrier
                    .gc_log
                    .map_or("unavailable".to_string(), |g| g.warn_or_error.to_string()),
            )?;
        }
        match &self.failure {
            Some(failure) => writeln!(f, "{failure}"),
            None => writeln!(f, "passed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn parses_seconds_minutes_and_hours() {
        assert_eq!(parse_duration("90s").unwrap(), Duration::from_secs(90));
        assert_eq!(parse_duration("30m").unwrap(), Duration::from_secs(1800));
        assert_eq!(parse_duration("4h").unwrap(), Duration::from_secs(14_400));
    }

    #[test]
    fn rejects_zero_missing_unit_unknown_unit_and_empty() {
        for raw in ["0m", "10", "5d", "", "m", "-3s", "1.5h"] {
            assert!(parse_duration(raw).is_err(), "{raw:?} must be rejected");
        }
    }

    const SKIPPED: &str = "\x1b[2m2026-11-19T10:00:00Z\x1b[0m \x1b[33m WARN\x1b[0m \x1b[2mprovider_garbage_collector\x1b[0m: GC reconcile skipped bucket_id=7 error=chain down";

    #[test]
    fn counts_skipped_reconciles_and_warnings_of_the_gc_only() {
        let text = [
            SKIPPED,
            "2026-11-19T10:00:01Z  WARN provider_garbage_collector: something else",
            "2026-11-19T10:00:02Z ERROR provider_garbage_collector: failed",
            "2026-11-19T10:00:03Z  INFO provider_garbage_collector: GC pass done",
            "2026-11-19T10:00:04Z  WARN provider_http: GC reconcile skipped (other target)",
            "2026-11-19T10:00:05Z  WARN other: unrelated",
        ]
        .join("\n");
        assert_eq!(
            count_gc_lines(&text),
            GcLogCounts {
                reconcile_skipped: 1,
                warn_or_error: 3
            }
        );
    }

    #[test]
    fn log_tail_reads_only_new_lines_and_restarts_after_truncation() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, "{SKIPPED}").unwrap();
        file.flush().unwrap();
        let mut tail = LogTail::open(file.path().to_path_buf()).unwrap();
        assert_eq!(tail.read_new().unwrap(), GcLogCounts::default());

        writeln!(file, "{SKIPPED}").unwrap();
        file.flush().unwrap();
        assert_eq!(tail.read_new().unwrap().reconcile_skipped, 1);
        assert_eq!(tail.read_new().unwrap(), GcLogCounts::default());

        file.as_file().set_len(0).unwrap();
        let mut reopened = std::fs::OpenOptions::new()
            .append(true)
            .open(file.path())
            .unwrap();
        writeln!(reopened, "{SKIPPED}").unwrap();
        assert_eq!(tail.read_new().unwrap().reconcile_skipped, 1);
    }

    #[test]
    fn local_port_accepts_only_this_machine() {
        assert_eq!(local_port("http://127.0.0.1:3333"), Some(3333));
        assert_eq!(local_port("http://localhost:3334/"), Some(3334));
        assert_eq!(local_port("http://[::1]:3335"), Some(3335));
        assert_eq!(local_port("http://provider.example:3333"), None);
        assert_eq!(local_port("http://127.0.0.1"), None);
    }

    #[tokio::test]
    async fn poll_until_returns_the_first_some() {
        let stop = StopSignal::new();
        let mut calls = 0;
        let value = poll_until(
            "a value",
            Duration::from_secs(5),
            Duration::from_millis(1),
            &stop,
            || {
                calls += 1;
                let ready = calls == 3;
                async move { Ok(ready.then_some(42)) }
            },
        )
        .await
        .unwrap();
        assert_eq!(value, 42);
        assert_eq!(calls, 3);
    }

    #[tokio::test]
    async fn poll_until_times_out_naming_what_it_waited_for() {
        let stop = StopSignal::new();
        let err = poll_until::<(), _, _>(
            "erasure",
            Duration::from_millis(20),
            Duration::from_millis(5),
            &stop,
            || async { Ok(None) },
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(err.contains("erasure"), "unexpected error: {err}");
    }

    #[tokio::test]
    async fn poll_until_stops_when_aborted() {
        let stop = StopSignal::new();
        stop.abort();
        let err = poll_until::<(), _, _>(
            "anything",
            Duration::from_secs(5),
            Duration::from_millis(5),
            &stop,
            || async { Ok(None) },
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(err.contains("aborted"), "unexpected error: {err}");
    }

    #[test]
    fn finish_keeps_work_running_and_abort_stops_it() {
        let stop = StopSignal::new();
        assert!(!stop.should_finish() && !stop.is_aborted());
        stop.finish();
        assert!(stop.should_finish() && !stop.is_aborted());
        stop.abort();
        assert!(stop.should_finish() && stop.is_aborted());
        stop.finish();
        assert!(stop.is_aborted(), "finish must not undo an abort");
    }

    fn report(failure: Option<SoakFailure>) -> SoakReport {
        let cycle = |epoch, cycle, secs| CycleRecord {
            lane: 0,
            epoch,
            cycle,
            bucket_id: 9,
            uploaded_bytes: 300,
            erase_latency_secs: secs,
            used_bytes_after: 100,
        };
        SoakReport {
            service: "gc",
            cycles: vec![cycle(0, 0, 2.0), cycle(0, 1, 4.0), cycle(1, 0, 8.0)],
            barriers: vec![
                BarrierRecord {
                    epoch: 0,
                    drain_secs: 16.5,
                    provider_rss_kb: Some(35_800),
                    gc_log: None,
                },
                BarrierRecord {
                    epoch: 1,
                    drain_secs: 18.0,
                    provider_rss_kb: None,
                    gc_log: Some(GcLogCounts::default()),
                },
            ],
            failure,
            elapsed: Duration::from_secs(60),
        }
    }

    #[test]
    fn epoch_summary_covers_only_that_epochs_cycles() {
        let summary = report(None).epoch_summary(0);
        assert_eq!(summary.total, 2);
        assert_eq!(summary.lat_min, Duration::from_secs(2));
        assert_eq!(summary.lat_max, Duration::from_secs(4));
    }

    #[test]
    fn text_report_lists_each_epoch_with_latency_drain_and_rss() {
        let text = report(None).to_string();
        assert!(text.contains("epoch 0"), "{text}");
        assert!(text.contains("epoch 1"), "{text}");
        assert!(text.contains("16.5"), "{text}");
        assert!(text.contains("35800"), "{text}");
        assert!(text.contains("unavailable"), "{text}");
        assert!(text.contains("p50"), "{text}");
        assert!(!text.contains("FAILED"), "{text}");
    }

    #[test]
    fn text_report_names_the_failure_location() {
        let failure = SoakFailure {
            message: "timed out".into(),
            epoch: 1,
            lane: Some(2),
            cycle: Some(3),
        };
        let text = report(Some(failure)).to_string();
        assert!(
            text.contains("FAILED epoch 1 lane 2 cycle 3: timed out"),
            "{text}"
        );
    }

    #[test]
    fn json_report_has_cycles_barriers_and_failure() {
        let failure = SoakFailure {
            message: "drain".into(),
            epoch: 1,
            lane: None,
            cycle: None,
        };
        let json: serde_json::Value =
            serde_json::from_str(&report(Some(failure)).to_json().unwrap()).unwrap();
        assert_eq!(json["cycles"].as_array().unwrap().len(), 3);
        assert_eq!(json["barriers"][0]["provider_rss_kb"], 35_800);
        assert!(json["barriers"][1]["provider_rss_kb"].is_null());
        assert_eq!(json["failure"]["epoch"], 1);
        assert!(json["failure"]["lane"].is_null());
    }
}
