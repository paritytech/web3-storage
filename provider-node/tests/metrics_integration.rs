// SPDX-License-Identifier: GPL-3.0-only

//! Integration tests for Prometheus metrics.
//!
//! Spins up the real router with a test registry and asserts that requests
//! land in the metric families via `Registry::gather()`.

mod common;

use axum::http::StatusCode;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use prometheus::proto::{MetricFamily, MetricType};
use prometheus::Registry;
use provider_storage::{NullNonceStore, Storage};
use reqwest::Method;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use storage_primitives::Role;
use storage_provider_node::auth::{MembershipCache, StaticMembershipResolver};
use storage_provider_node::{
    ProviderDeps, ProviderMetrics, ProviderState, StorageMetricsCollector,
};

/// Server with metrics wired to a caller-owned registry.
async fn serve_with_metrics() -> (std::net::SocketAddr, common::SignedClient, Registry) {
    let storage = Arc::new(Storage::new());
    let registry = Registry::new();
    let metrics = ProviderMetrics::register(&registry);
    assert!(metrics.is_some(), "fresh registry must register");
    StorageMetricsCollector::register(&registry, storage.clone());

    let deps = ProviderDeps {
        storage,
        nonce_store: Arc::new(NullNonceStore),
        membership: Arc::new(MembershipCache::new(
            Box::new(StaticMembershipResolver(vec![(
                common::test_member_account(),
                Role::Admin,
            )])),
            Duration::from_secs(60),
        )),
        auth_max_skew: Duration::from_secs(300),
    };
    let state = ProviderState::with_seed(deps, common::TEST_MEMBER_SEED)
        .expect("//Alice is a valid SURI")
        .with_metrics(metrics);

    let (addr, client) = common::serve(state).await;
    (addr, client, registry)
}

fn family<'a>(families: &'a [MetricFamily], name: &str) -> &'a MetricFamily {
    families
        .iter()
        .find(|f| f.get_name() == name)
        .unwrap_or_else(|| panic!("missing family {name}"))
}

/// Value of the series in `name` whose labels contain all `labels` pairs.
fn series_value(families: &[MetricFamily], name: &str, labels: &[(&str, &str)]) -> f64 {
    let fam = family(families, name);
    let metric = fam
        .get_metric()
        .iter()
        .find(|m| {
            labels.iter().all(|(k, v)| {
                m.get_label()
                    .iter()
                    .any(|l| l.get_name() == *k && l.get_value() == *v)
            })
        })
        .unwrap_or_else(|| panic!("no series in {name} matching {labels:?}"));
    match fam.get_field_type() {
        MetricType::COUNTER => metric.get_counter().get_value(),
        _ => metric.get_gauge().get_value(),
    }
}

fn counter_value(families: &[MetricFamily], name: &str) -> f64 {
    family(families, name).get_metric()[0]
        .get_counter()
        .get_value()
}

#[tokio::test]
async fn middleware_and_handlers_record_metrics() {
    let (addr, client, registry) = serve_with_metrics().await;
    let url = |path: &str| format!("http://{addr}{path}");

    // Upload one chunk (18 bytes), commit it, then read it back twice
    // (GET /node and GET /read).
    let data = b"metrics chunk data";
    let hash_hex = format!(
        "0x{}",
        hex::encode(storage_primitives::blake2_256(data).as_bytes())
    );
    let resp = client
        .put(url("/node"))
        .json(&json!({
            "bucket_id": 1,
            "hash": hash_hex,
            "data": BASE64.encode(data),
            "children": null
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = client
        .post(url("/commit"))
        .json(&json!({ "bucket_id": 1, "data_roots": [hash_hex], "nonce": 0u64 }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = client
        .get(url(&format!("/node?hash={hash_hex}")))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = client
        .get(url(&format!(
            "/read?data_root={hash_hex}&offset=0&length={}",
            data.len()
        )))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // S3 and FS puts + gets (15 bytes each way, each).
    let s3_body = b"s3 metrics body";
    let resp = client
        .put(url("/s3/1/object?key=m.txt"))
        .body(s3_body.to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = client
        .get(url("/s3/1/object?key=m.txt"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let fs_body = b"fs metrics body";
    let resp = client
        .put(url("/fs/1/file?path=/m.txt"))
        .body(fs_body.to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = client
        .get(url("/fs/1/file?path=/m.txt"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Commit against an unknown bucket: a storage-engine error surfacing as 404.
    let resp = client
        .request_bucket(Method::POST, url("/commit"), 999)
        .json(&json!({ "bucket_id": 999, "data_roots": [hash_hex], "nonce": 1u64 }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // Unrouted path: must land under the fixed "unmatched" label, not the raw URI.
    let resp = client
        .get(url("/definitely-not-a-route"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let families = registry.gather();

    // Route labels are matched-path templates, methods and statuses split series.
    for (labels, expected) in [
        (
            [("route", "/node"), ("method", "PUT"), ("status", "200")],
            1.0,
        ),
        (
            [("route", "/node"), ("method", "GET"), ("status", "200")],
            1.0,
        ),
        (
            [("route", "/read"), ("method", "GET"), ("status", "200")],
            1.0,
        ),
        (
            [("route", "/commit"), ("method", "POST"), ("status", "200")],
            1.0,
        ),
        (
            [("route", "/commit"), ("method", "POST"), ("status", "404")],
            1.0,
        ),
        (
            [
                ("route", "/s3/:bucket_id/object"),
                ("method", "PUT"),
                ("status", "200"),
            ],
            1.0,
        ),
        (
            [
                ("route", "/fs/:bucket_id/file"),
                ("method", "PUT"),
                ("status", "200"),
            ],
            1.0,
        ),
        (
            [("route", "unmatched"), ("method", "GET"), ("status", "404")],
            1.0,
        ),
    ] {
        assert_eq!(
            series_value(&families, "http_requests_total", &labels),
            expected,
            "unexpected count for {labels:?}"
        );
    }

    // Latency histogram exists per route; sample count matches request count.
    let duration = family(&families, "http_request_duration_seconds");
    let node_series = duration
        .get_metric()
        .iter()
        .find(|m| m.get_label().iter().any(|l| l.get_value() == "/node"))
        .expect("duration series for /node");
    assert_eq!(node_series.get_histogram().get_sample_count(), 2);

    // All requests completed, so the in-flight gauge must be back to zero.
    assert_eq!(series_value(&families, "http_requests_in_flight", &[]), 0.0);

    // Uploads: /node chunk + s3 body + fs body.
    assert_eq!(
        counter_value(&families, "upload_bytes_total"),
        (data.len() + s3_body.len() + fs_body.len()) as f64
    );
    // Downloads: GET /node + GET /read + s3 get + fs get.
    assert_eq!(
        counter_value(&families, "download_bytes_total"),
        (2 * data.len() + s3_body.len() + fs_body.len()) as f64
    );

    // The failed commit is attributed to the engine variant, not the message.
    assert_eq!(
        series_value(
            &families,
            "storage_errors_total",
            &[("op", "commit"), ("reason", "bucket_not_found")]
        ),
        1.0
    );

    // Engine commits: POST /commit + the s3 and fs put flows.
    assert_eq!(
        family(&families, "commit_duration_seconds").get_metric()[0]
            .get_histogram()
            .get_sample_count(),
        3
    );

    // Scrape-time backend gauges: one chunk node per upload path.
    assert!(series_value(&families, "storage_nodes", &[]) >= 3.0);
    assert!(
        series_value(&families, "storage_bytes", &[])
            >= (data.len() + s3_body.len() + fs_body.len()) as f64
    );
    assert!(series_value(&families, "process_start_time_seconds", &[]) > 0.0);
}

#[tokio::test]
async fn disabled_metrics_do_not_break_requests() {
    // No `with_metrics`: every handler and the middleware must no-op cleanly.
    let deps = ProviderDeps {
        storage: Arc::new(Storage::new()),
        nonce_store: Arc::new(NullNonceStore),
        membership: Arc::new(MembershipCache::new(
            Box::new(StaticMembershipResolver(vec![(
                common::test_member_account(),
                Role::Admin,
            )])),
            Duration::from_secs(60),
        )),
        auth_max_skew: Duration::from_secs(300),
    };
    let state = ProviderState::with_seed(deps, common::TEST_MEMBER_SEED).unwrap();
    let (addr, client) = common::serve(state).await;

    let resp = client
        .get(format!("http://{addr}/health"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}
