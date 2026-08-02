// SPDX-License-Identifier: GPL-3.0-only

//! Prometheus metrics for the provider node.
//!
//! Families live in a node-local [`Registry`] served by
//! `substrate-prometheus-endpoint` on its own listener (see `--prometheus-port`),
//! never on the public API router.

use crate::error::Error;
use crate::ProviderState;
use axum::{
    extract::{MatchedPath, Request, State},
    middleware::Next,
    response::Response,
};
use prometheus::{
    core::{Collector, Desc},
    proto::MetricFamily,
    Gauge, Histogram, HistogramOpts, HistogramVec, IntCounter, IntCounterVec, IntGauge, Opts,
    Registry,
};
use provider_storage::StorageBackend;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// HTTP and data-path metrics recorded by the API layer.
pub struct ProviderMetrics {
    /// Completed HTTP requests by matched route, method, and status.
    pub http_requests_total: IntCounterVec,
    /// HTTP request latency by matched route.
    pub http_request_duration_seconds: HistogramVec,
    /// Requests currently being served.
    pub http_requests_in_flight: IntGauge,
    /// Payload bytes accepted by upload handlers.
    pub upload_bytes_total: IntCounter,
    /// Payload bytes served by download handlers.
    pub download_bytes_total: IntCounter,
    /// Storage-engine failures by operation and error variant.
    pub storage_errors_total: IntCounterVec,
    /// Duration of storage-engine MMR commits.
    pub commit_duration_seconds: Histogram,
}

impl ProviderMetrics {
    /// Register all families on `registry`. A failure only disables metrics
    /// (returns `None`) so the node keeps serving without them.
    pub fn register(registry: &Registry) -> Option<Self> {
        match Self::try_register(registry) {
            Ok(metrics) => Some(metrics),
            Err(e) => {
                tracing::warn!("Failed to register Prometheus metrics ({e}); metrics disabled");
                None
            }
        }
    }

    fn try_register(registry: &Registry) -> Result<Self, prometheus::Error> {
        let http_requests_total = IntCounterVec::new(
            Opts::new(
                "http_requests_total",
                "Completed HTTP requests by matched route, method, and status.",
            ),
            &["route", "method", "status"],
        )?;
        registry.register(Box::new(http_requests_total.clone()))?;

        let http_request_duration_seconds = HistogramVec::new(
            HistogramOpts::new(
                "http_request_duration_seconds",
                "HTTP request latency by matched route.",
            )
            // Upper buckets sized for multi-hundred-MB uploads (body limit is
            // 256 MB), which legitimately take tens of seconds.
            .buckets(vec![
                0.005, 0.02, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0,
            ]),
            &["route"],
        )?;
        registry.register(Box::new(http_request_duration_seconds.clone()))?;

        let http_requests_in_flight = IntGauge::new(
            "http_requests_in_flight",
            "Requests currently being served.",
        )?;
        registry.register(Box::new(http_requests_in_flight.clone()))?;

        let upload_bytes_total = IntCounter::new(
            "upload_bytes_total",
            "Payload bytes accepted by upload handlers.",
        )?;
        registry.register(Box::new(upload_bytes_total.clone()))?;

        let download_bytes_total = IntCounter::new(
            "download_bytes_total",
            "Payload bytes served by download handlers.",
        )?;
        registry.register(Box::new(download_bytes_total.clone()))?;

        let storage_errors_total = IntCounterVec::new(
            Opts::new(
                "storage_errors_total",
                "Storage-engine failures by operation and error variant.",
            ),
            &["op", "reason"],
        )?;
        registry.register(Box::new(storage_errors_total.clone()))?;

        let commit_duration_seconds = Histogram::with_opts(
            HistogramOpts::new(
                "commit_duration_seconds",
                "Duration of storage-engine MMR commits.",
            )
            .buckets(vec![0.001, 0.005, 0.025, 0.1, 0.25, 1.0, 2.5, 10.0]),
        )?;
        registry.register(Box::new(commit_duration_seconds.clone()))?;

        Ok(Self {
            http_requests_total,
            http_request_duration_seconds,
            http_requests_in_flight,
            upload_bytes_total,
            download_bytes_total,
            storage_errors_total,
            commit_duration_seconds,
        })
    }

    /// Count a storage-engine failure under a fixed `op` label.
    pub fn observe_storage_error(&self, op: &str, err: &provider_storage::Error) {
        self.storage_errors_total
            .with_label_values(&[op, storage_error_reason(err)])
            .inc();
    }
}

/// Fixed low-cardinality `reason` label: one value per engine error variant,
/// never the error message.
fn storage_error_reason(err: &provider_storage::Error) -> &'static str {
    use provider_storage::Error as E;
    match err {
        E::NodeNotFound(_) => "node_not_found",
        E::ChildrenMissing(_) => "children_missing",
        E::QuotaExceeded { .. } => "quota_exceeded",
        E::BucketNotFound(_) => "bucket_not_found",
        E::RootNotFound(_) => "root_not_found",
        E::InvalidHash { .. } => "invalid_hash",
        E::Storage(_) => "storage",
        E::Serialization(_) => "serialization",
    }
}

/// Recording helpers for handlers; all no-ops when metrics are disabled.
impl ProviderState {
    /// Record a storage-engine failure before converting it into the HTTP
    /// error type, which erases the variant.
    pub(crate) fn storage_err(&self, op: &'static str, err: provider_storage::Error) -> Error {
        if let Some(metrics) = &self.metrics {
            metrics.observe_storage_error(op, &err);
        }
        err.into()
    }

    pub(crate) fn count_upload_bytes(&self, n: u64) {
        if let Some(metrics) = &self.metrics {
            metrics.upload_bytes_total.inc_by(n);
        }
    }

    pub(crate) fn count_download_bytes(&self, n: u64) {
        if let Some(metrics) = &self.metrics {
            metrics.download_bytes_total.inc_by(n);
        }
    }

    pub(crate) fn observe_commit_duration(&self, started: Instant) {
        if let Some(metrics) = &self.metrics {
            metrics
                .commit_duration_seconds
                .observe(started.elapsed().as_secs_f64());
        }
    }
}

/// Record count, latency, and in-flight gauge for every request.
pub async fn track_http_metrics(
    State(state): State<Arc<ProviderState>>,
    request: Request,
    next: Next,
) -> Response {
    let Some(metrics) = &state.metrics else {
        return next.run(request).await;
    };

    // Label with the route template (`/s3/:bucket_id/object`), never the raw
    // URI: raw paths and object keys would blow up label cardinality.
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map(|p| p.as_str().to_owned())
        .unwrap_or_else(|| "unmatched".to_owned());
    let method = request.method().as_str().to_owned();

    metrics.http_requests_in_flight.inc();
    let started = Instant::now();
    let response = next.run(request).await;
    metrics.http_requests_in_flight.dec();

    metrics
        .http_request_duration_seconds
        .with_label_values(&[&route])
        .observe(started.elapsed().as_secs_f64());
    metrics
        .http_requests_total
        .with_label_values(&[&route, &method, response.status().as_str()])
        .inc();

    response
}

/// Scrape-time gauges read straight from the storage backend, so totals stay
/// correct without instrumenting every mutation path.
pub struct StorageMetricsCollector {
    storage: Arc<dyn StorageBackend>,
    /// Set from `total_nodes()` at scrape time.
    storage_nodes: IntGauge,
    /// Set from `total_bytes()` at scrape time.
    storage_bytes: IntGauge,
    /// Constant. Provided here because the node registry is not the default
    /// one, so nothing else exports it.
    process_start_time_seconds: Gauge,
}

impl StorageMetricsCollector {
    pub fn new(storage: Arc<dyn StorageBackend>) -> Result<Self, prometheus::Error> {
        let storage_nodes = IntGauge::new(
            "storage_nodes",
            "Nodes currently held by the storage backend.",
        )?;
        let storage_bytes = IntGauge::new(
            "storage_bytes",
            "Bytes currently held by the storage backend.",
        )?;
        let process_start_time_seconds = Gauge::new(
            "process_start_time_seconds",
            "Unix time the provider process started.",
        )?;
        // Set once here: metrics are constructed during node startup, so this
        // approximates the process start closely enough.
        process_start_time_seconds.set(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0),
        );
        Ok(Self {
            storage,
            storage_nodes,
            storage_bytes,
            process_start_time_seconds,
        })
    }

    /// Best-effort registration; a failure only loses these gauges.
    pub fn register(registry: &Registry, storage: Arc<dyn StorageBackend>) {
        let collector = match Self::new(storage) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("Failed to create storage metrics collector: {e}");
                return;
            }
        };
        if let Err(e) = registry.register(Box::new(collector)) {
            tracing::warn!("Failed to register storage metrics collector: {e}");
        }
    }
}

impl Collector for StorageMetricsCollector {
    fn desc(&self) -> Vec<&Desc> {
        self.storage_nodes
            .desc()
            .into_iter()
            .chain(self.storage_bytes.desc())
            .chain(self.process_start_time_seconds.desc())
            .collect()
    }

    fn collect(&self) -> Vec<MetricFamily> {
        self.storage_nodes.set(self.storage.total_nodes() as i64);
        self.storage_bytes.set(self.storage.total_bytes() as i64);
        let mut families = self.storage_nodes.collect();
        families.extend(self.storage_bytes.collect());
        families.extend(self.process_start_time_seconds.collect());
        families
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use provider_storage::Storage;

    fn family<'a>(families: &'a [MetricFamily], name: &str) -> Option<&'a MetricFamily> {
        families.iter().find(|f| f.get_name() == name)
    }

    #[test]
    fn register_exposes_all_families_after_use() {
        let registry = Registry::new();
        let metrics = ProviderMetrics::register(&registry).expect("fresh registry registers");

        // Vec metrics only appear in gather() once a child exists.
        metrics
            .http_requests_total
            .with_label_values(&["/health", "GET", "200"])
            .inc();
        metrics
            .http_request_duration_seconds
            .with_label_values(&["/health"])
            .observe(0.01);
        metrics.http_requests_in_flight.set(2);
        metrics.upload_bytes_total.inc_by(10);
        metrics.download_bytes_total.inc_by(20);
        metrics.observe_storage_error("commit", &provider_storage::Error::BucketNotFound(7));
        metrics.commit_duration_seconds.observe(0.5);

        let families = registry.gather();
        for name in [
            "http_requests_total",
            "http_request_duration_seconds",
            "http_requests_in_flight",
            "upload_bytes_total",
            "download_bytes_total",
            "storage_errors_total",
            "commit_duration_seconds",
        ] {
            assert!(family(&families, name).is_some(), "missing family {name}");
        }

        let errors = family(&families, "storage_errors_total").unwrap();
        let labels = errors.get_metric()[0].get_label();
        let mut pairs: Vec<(&str, &str)> = labels
            .iter()
            .map(|l| (l.get_name(), l.get_value()))
            .collect();
        pairs.sort();
        assert_eq!(
            pairs,
            vec![("op", "commit"), ("reason", "bucket_not_found")]
        );
    }

    #[test]
    fn register_twice_on_same_registry_disables_metrics() {
        let registry = Registry::new();
        assert!(ProviderMetrics::register(&registry).is_some());
        // Duplicate registration must degrade to None, not panic.
        assert!(ProviderMetrics::register(&registry).is_none());
    }

    #[test]
    fn storage_error_reason_covers_all_variants() {
        use provider_storage::Error as E;
        let cases = [
            (E::NodeNotFound("h".into()), "node_not_found"),
            (E::ChildrenMissing(vec![]), "children_missing"),
            (E::QuotaExceeded { used: 1, max: 2 }, "quota_exceeded"),
            (E::BucketNotFound(1), "bucket_not_found"),
            (E::RootNotFound("r".into()), "root_not_found"),
            (
                E::InvalidHash {
                    expected: "a".into(),
                    actual: "b".into(),
                },
                "invalid_hash",
            ),
            (E::Storage("s".into()), "storage"),
            (E::Serialization("s".into()), "serialization"),
        ];
        for (err, reason) in cases {
            assert_eq!(storage_error_reason(&err), reason);
        }
    }

    #[test]
    fn collector_reflects_backend_totals_at_scrape_time() {
        let storage = Arc::new(Storage::new());
        let registry = Registry::new();
        StorageMetricsCollector::register(&registry, storage.clone());

        let read_gauge = |name: &str| -> f64 {
            let families = registry.gather();
            family(&families, name).unwrap().get_metric()[0]
                .get_gauge()
                .get_value()
        };

        assert_eq!(read_gauge("storage_nodes"), 0.0);
        assert_eq!(read_gauge("storage_bytes"), 0.0);
        assert!(read_gauge("process_start_time_seconds") > 0.0);

        // Mutate the backend after registration: the next scrape must see it.
        storage.init_bucket(1, u64::MAX);
        let data = b"metrics test data".to_vec();
        let hash = storage_primitives::blake2_256(&data);
        storage.store_node(1, hash, data.clone(), None).unwrap();

        assert_eq!(read_gauge("storage_nodes"), 1.0);
        assert_eq!(read_gauge("storage_bytes"), data.len() as f64);
    }
}
