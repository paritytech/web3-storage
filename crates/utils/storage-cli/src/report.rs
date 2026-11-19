// SPDX-License-Identifier: Apache-2.0

//! The `--report` file: one JSON line per record, written as it happens, so a
//! run that stops unexpectedly keeps every line written before it stopped.

use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::Serialize;

use crate::common::BucketId;
use crate::metrics::OpOutcome;
use crate::soak::{BarrierRecord, CycleRecord, SoakFailure};

/// A hex timestamp that identifies one invocation in a report file.
pub fn new_run_id() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    format!("{millis:x}")
}

/// One upload of `stress-test upload`.
#[derive(Debug, Clone, Serialize)]
pub struct UploadRecord {
    pub user: usize,
    pub bucket_id: BucketId,
    pub ok: bool,
    pub bytes: usize,
    pub elapsed_secs: f64,
    pub error: Option<String>,
}

impl UploadRecord {
    pub fn new(user: usize, bucket_id: BucketId, outcome: &OpOutcome) -> Self {
        Self {
            user,
            bucket_id,
            ok: outcome.ok,
            bytes: outcome.bytes,
            elapsed_secs: outcome.elapsed.as_secs_f64(),
            error: outcome.err.clone(),
        }
    }
}

/// One line of the `--report` file.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReportLine {
    Start {
        command: &'static str,
        /// Unix seconds.
        started_at: u64,
    },
    Cycle(CycleRecord),
    Barrier(BarrierRecord),
    Failure(SoakFailure),
    Upload(UploadRecord),
    End {
        elapsed_secs: f64,
        error: Option<String>,
    },
}

/// A report line with the run it belongs to.
#[derive(Serialize)]
struct ReportEntry<'a> {
    run_id: &'a str,
    #[serde(flatten)]
    line: &'a ReportLine,
}

/// Appends report lines to a file as they happen.
pub struct ReportFile {
    run_id: String,
    file: Mutex<std::fs::File>,
}

impl ReportFile {
    /// Open `path` for appending, creating it if missing.
    pub fn open(path: &Path, run_id: &str) -> Result<Self> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .with_context(|| format!("failed to open report file {}", path.display()))?;
        Ok(Self {
            run_id: run_id.to_string(),
            file: Mutex::new(file),
        })
    }

    /// Write `line` as one JSON object with `run_id` and `kind`.
    pub fn append(&self, line: &ReportLine) -> Result<()> {
        let mut bytes = serde_json::to_vec(&ReportEntry {
            run_id: &self.run_id,
            line,
        })?;
        bytes.push(b'\n');
        self.file
            .lock()
            .unwrap()
            .write_all(&bytes)
            .context("failed to write to the report file")
    }
}

/// What identifies one invocation: its id and, with `--report`, the file.
pub struct Run {
    pub id: String,
    pub report: Option<Arc<ReportFile>>,
}

impl Run {
    /// A run with a new id, opening the report file when a path is given.
    pub fn open(report: Option<&Path>) -> Result<Self> {
        let id = new_run_id();
        let report = report
            .map(|path| ReportFile::open(path, &id).map(Arc::new))
            .transpose()?;
        Ok(Self { id, report })
    }

    /// Append `line` when a report file is open.
    pub fn record(&self, line: &ReportLine) -> Result<()> {
        match &self.report {
            Some(report) => report.append(line),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
impl Run {
    /// A run whose report file rejects every write.
    pub fn failing() -> Self {
        Self {
            id: "failing".to_string(),
            report: Some(Arc::new(ReportFile {
                run_id: "failing".to_string(),
                file: Mutex::new(std::fs::File::open("/dev/null").unwrap()),
            })),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn lines(path: &Path) -> Vec<serde_json::Value> {
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn cycle() -> CycleRecord {
        CycleRecord {
            lane: 1,
            epoch: 2,
            cycle: 3,
            bucket_id: 9,
            uploaded_bytes: 300,
            erase_latency_secs: 1.5,
            used_bytes_after: 100,
        }
    }

    #[test]
    fn every_line_is_a_json_object_with_run_id_kind_and_the_records_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("report.jsonl");
        let report = ReportFile::open(&path, "r1").unwrap();
        let failure = SoakFailure {
            message: "drain".into(),
            epoch: 4,
            lane: None,
            cycle: None,
        };
        let upload = UploadRecord::new(
            2,
            7,
            &OpOutcome::failure(10, Duration::from_millis(1500), "boom".into()),
        );
        for line in [
            ReportLine::Start {
                command: "soak gc",
                started_at: 1_700_000_000,
            },
            ReportLine::Cycle(cycle()),
            ReportLine::Barrier(BarrierRecord {
                epoch: 2,
                drain_secs: 6.0,
                provider_rss_kb: None,
                gc_log: None,
            }),
            ReportLine::Failure(failure),
            ReportLine::Upload(upload),
            ReportLine::End {
                elapsed_secs: 12.5,
                error: Some("x".into()),
            },
        ] {
            report.append(&line).unwrap();
        }

        let got = lines(&path);
        let kinds: Vec<&str> = got.iter().map(|l| l["kind"].as_str().unwrap()).collect();
        assert_eq!(
            kinds,
            ["start", "cycle", "barrier", "failure", "upload", "end"]
        );
        assert!(got.iter().all(|l| l["run_id"] == "r1"));
        assert_eq!(got[0]["command"], "soak gc");
        assert_eq!(got[0]["started_at"], 1_700_000_000u64);
        assert_eq!(got[1]["bucket_id"], 9);
        assert_eq!(got[1]["erase_latency_secs"], 1.5);
        assert_eq!(got[2]["drain_secs"], 6.0);
        assert!(got[2]["provider_rss_kb"].is_null());
        assert_eq!(got[3]["epoch"], 4);
        assert_eq!(got[3]["message"], "drain");
        assert_eq!(got[4]["user"], 2);
        assert_eq!(got[4]["ok"], false);
        assert_eq!(got[4]["elapsed_secs"], 1.5);
        assert_eq!(got[4]["error"], "boom");
        assert_eq!(got[5]["error"], "x");
    }

    #[test]
    fn a_line_is_on_disk_while_the_writer_is_still_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("report.jsonl");
        let report = ReportFile::open(&path, "r1").unwrap();

        report.append(&ReportLine::Cycle(cycle())).unwrap();
        assert_eq!(lines(&path).len(), 1);
        report.append(&ReportLine::Cycle(cycle())).unwrap();

        assert_eq!(lines(&path).len(), 2);
        drop(report);
    }

    #[test]
    fn reopening_a_report_appends_after_the_earlier_run() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("report.jsonl");
        ReportFile::open(&path, "first")
            .unwrap()
            .append(&ReportLine::Cycle(cycle()))
            .unwrap();

        ReportFile::open(&path, "second")
            .unwrap()
            .append(&ReportLine::Cycle(cycle()))
            .unwrap();

        let got = lines(&path);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0]["run_id"], "first");
        assert_eq!(got[1]["run_id"], "second");
    }

    #[test]
    fn opening_a_path_in_a_missing_directory_names_the_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing").join("report.jsonl");
        let err = ReportFile::open(&path, "r1")
            .err()
            .expect("open must fail")
            .to_string();
        assert!(err.contains("missing"), "unexpected error: {err}");
    }

    #[test]
    fn upload_record_copies_the_outcome() {
        let ok = UploadRecord::new(0, 5, &OpOutcome::success(64, Duration::from_millis(250)));
        assert!(ok.ok && ok.error.is_none());
        assert_eq!(
            (ok.user, ok.bucket_id, ok.bytes, ok.elapsed_secs),
            (0, 5, 64, 0.25)
        );
    }
}
