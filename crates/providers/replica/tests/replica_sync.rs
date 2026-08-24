// SPDX-License-Identifier: Apache-2.0

//! Integration tests for the replica sync coordinator.

use axum::{routing::get, Json, Router};
use provider_replica::coordinator::{BucketSnapshot, ReplicaAgreementInfo};
use provider_replica::{
    Error, ReplicaSyncChainClient, ReplicaSyncCoordinator, ReplicaSyncCoordinatorConfig, SyncDuty,
    SyncResult,
};
use provider_storage::{temp_rocksdb, StorageBackend};
use sp_core::H256;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use storage_primitives::{blake2_256, BucketId};
use tempfile::TempDir;

/// Full Alice SS58 address (substrate prefix 42).
const ALICE_SS58: &str = "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY";

/// Fresh empty storage backend for coordinator tests. The returned `TempDir`
/// must outlive the backend - dropping it takes the database with it.
fn test_storage() -> (Arc<dyn StorageBackend>, TempDir) {
    let (storage, _nonce_store, dir) = temp_rocksdb();
    (storage, dir)
}

struct MockReplicaSyncChainClient {
    block: Mutex<u64>,
    agreements: Mutex<Vec<ReplicaAgreementInfo>>,
    snapshots: Mutex<HashMap<BucketId, BucketSnapshot>>,
    endpoints: Mutex<HashMap<BucketId, Vec<String>>>,
    confirmations: Mutex<Vec<BucketId>>,
    confirm_result: Mutex<Result<(u8, u128), Error>>,
}

impl MockReplicaSyncChainClient {
    fn new() -> Self {
        Self {
            block: Mutex::new(100),
            agreements: Mutex::new(Vec::new()),
            snapshots: Mutex::new(HashMap::new()),
            endpoints: Mutex::new(HashMap::new()),
            confirmations: Mutex::new(Vec::new()),
            confirm_result: Mutex::new(Ok((0, 1000))),
        }
    }

    fn with_agreements(self, agreements: Vec<ReplicaAgreementInfo>) -> Self {
        Self {
            agreements: Mutex::new(agreements),
            ..self
        }
    }

    fn with_snapshot(self, bucket_id: BucketId, snapshot: BucketSnapshot) -> Self {
        let mut map = self.snapshots.into_inner().unwrap();
        map.insert(bucket_id, snapshot);
        Self {
            snapshots: Mutex::new(map),
            ..self
        }
    }

    fn with_endpoints(self, bucket_id: BucketId, endpoints: Vec<String>) -> Self {
        let mut map = self.endpoints.into_inner().unwrap();
        map.insert(bucket_id, endpoints);
        Self {
            endpoints: Mutex::new(map),
            ..self
        }
    }
}

#[async_trait::async_trait]
impl ReplicaSyncChainClient for MockReplicaSyncChainClient {
    async fn get_current_block(&self) -> Result<u64, Error> {
        Ok(*self.block.lock().unwrap())
    }

    async fn fetch_replica_agreements(
        &self,
        _provider_account: &str,
        _local_buckets: Vec<BucketId>,
    ) -> Result<Vec<ReplicaAgreementInfo>, Error> {
        Ok(self.agreements.lock().unwrap().clone())
    }

    async fn fetch_bucket_snapshot(&self, bucket_id: BucketId) -> Result<BucketSnapshot, Error> {
        let snapshots = self.snapshots.lock().unwrap();
        Ok(snapshots
            .get(&bucket_id)
            .cloned()
            .unwrap_or(BucketSnapshot {
                mmr_root: H256::zero(),
                leaf_count: 0,
            }))
    }

    async fn fetch_primary_endpoints(&self, bucket_id: BucketId) -> Result<Vec<String>, Error> {
        let endpoints = self.endpoints.lock().unwrap();
        Ok(endpoints.get(&bucket_id).cloned().unwrap_or_default())
    }

    async fn submit_sync_confirmation(
        &self,
        bucket_id: BucketId,
        _target_mmr_root: H256,
    ) -> Result<(u8, u128), Error> {
        self.confirmations.lock().unwrap().push(bucket_id);
        let result = &*self.confirm_result.lock().unwrap();
        match result {
            Ok(v) => Ok(*v),
            Err(e) => Err(Error::Internal(e.to_string())),
        }
    }
}

#[test]
fn test_config_default() {
    let config = ReplicaSyncCoordinatorConfig::default();
    assert_eq!(config.poll_interval, Duration::from_secs(600));
    assert_eq!(config.max_concurrent_syncs, 3);
    assert!(config.auto_confirm);
}

#[tokio::test]
async fn test_no_agreements() {
    let mock = MockReplicaSyncChainClient::new();
    let config = ReplicaSyncCoordinatorConfig::default();
    let (storage, _dir) = test_storage();
    let coordinator =
        ReplicaSyncCoordinator::new(config, storage, ALICE_SS58.to_string(), Box::new(mock));

    let duties = coordinator.get_active_replica_duties().await.unwrap();
    assert!(duties.is_empty());
}

#[tokio::test]
async fn test_insufficient_balance() {
    let duty = SyncDuty {
        bucket_id: 1,
        target_mmr_root: H256::repeat_byte(0xAA),
        target_leaf_count: 10,
        primary_endpoints: vec![],
        sync_balance: 50,
        sync_price: 100,
        min_sync_interval: 0,
        last_sync: None,
    };

    let mock = MockReplicaSyncChainClient::new();
    let config = ReplicaSyncCoordinatorConfig::default();
    let (storage, _dir) = test_storage();
    let coordinator =
        ReplicaSyncCoordinator::new(config, storage, ALICE_SS58.to_string(), Box::new(mock));

    let result = coordinator.sync_and_confirm(&duty).await;
    assert!(matches!(result, SyncResult::InsufficientBalance { .. }));
}

#[tokio::test]
async fn test_already_synced() {
    let (storage, _nonce_store, _dir) = temp_rocksdb();
    storage
        .init_bucket(1, u64::MAX)
        .expect("bucket initialises");
    let data = b"test data".to_vec();
    let data_root = blake2_256(&data);
    let _ = storage.store_node(1, data_root, data, None);
    let (mmr_root, _, _) = storage.commit(1, vec![data_root]).unwrap();

    let duty = SyncDuty {
        bucket_id: 1,
        target_mmr_root: mmr_root,
        target_leaf_count: 1,
        primary_endpoints: vec![],
        sync_balance: 1000,
        sync_price: 100,
        min_sync_interval: 0,
        last_sync: None,
    };

    let mock = MockReplicaSyncChainClient::new();
    let config = ReplicaSyncCoordinatorConfig::default();
    let coordinator =
        ReplicaSyncCoordinator::new(config, storage, "test".to_string(), Box::new(mock));

    let result = coordinator.sync_and_confirm(&duty).await;
    assert!(matches!(result, SyncResult::AlreadySynced { .. }));
}

#[tokio::test]
async fn test_no_data_to_sync() {
    let duty = SyncDuty {
        bucket_id: 1,
        target_mmr_root: H256::zero(),
        target_leaf_count: 0,
        primary_endpoints: vec![],
        sync_balance: 1000,
        sync_price: 100,
        min_sync_interval: 0,
        last_sync: None,
    };

    let mock = MockReplicaSyncChainClient::new();
    let config = ReplicaSyncCoordinatorConfig::default();
    let (storage, _dir) = test_storage();
    let coordinator =
        ReplicaSyncCoordinator::new(config, storage, ALICE_SS58.to_string(), Box::new(mock));

    let result = coordinator.sync_and_confirm(&duty).await;
    assert!(matches!(result, SyncResult::NoDataToSync { .. }));
}

#[tokio::test]
async fn test_primary_unavailable() {
    let duty = SyncDuty {
        bucket_id: 1,
        target_mmr_root: H256::repeat_byte(0xAA),
        target_leaf_count: 10,
        primary_endpoints: vec!["http://127.0.0.1:19999".to_string()],
        sync_balance: 1000,
        sync_price: 100,
        min_sync_interval: 0,
        last_sync: None,
    };

    let mock = MockReplicaSyncChainClient::new();
    let config = ReplicaSyncCoordinatorConfig::default();
    let (storage, _dir) = test_storage();
    let coordinator =
        ReplicaSyncCoordinator::new(config, storage, ALICE_SS58.to_string(), Box::new(mock));

    let result = coordinator.sync_and_confirm(&duty).await;
    assert!(matches!(result, SyncResult::PrimaryUnavailable { .. }));
}

/// Spawn a minimal mock primary that answers `GET /mmr_peaks` with a fixed
/// root and no peaks, so `sync_and_confirm` completes the HTTP round trip and
/// reaches its post-sync verification step.
async fn spawn_mock_primary(bucket_id: BucketId, mmr_root_hex: String) -> String {
    let app = Router::new().route(
        "/mmr_peaks",
        get(move || {
            let mmr_root_hex = mmr_root_hex.clone();
            async move {
                Json(serde_json::json!({
                    "bucket_id": bucket_id,
                    "mmr_root": mmr_root_hex,
                    "peaks": Vec::<String>::new(),
                }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    while tokio::net::TcpStream::connect(addr).await.is_err() {
        tokio::task::yield_now().await;
    }
    format!("http://{addr}")
}

#[tokio::test]
async fn test_sync_from_primary_succeeds_but_final_verification_fails() {
    let bucket_id = 1;
    let target_root = H256::repeat_byte(0xDD);
    let target_root_hex = format!("0x{}", hex::encode(target_root.as_bytes()));
    let primary_url = spawn_mock_primary(bucket_id, target_root_hex).await;

    let duty = SyncDuty {
        bucket_id,
        target_mmr_root: target_root,
        target_leaf_count: 0,
        primary_endpoints: vec![primary_url],
        sync_balance: 1000,
        sync_price: 100,
        min_sync_interval: 0,
        last_sync: None,
    };

    let mock = MockReplicaSyncChainClient::new();
    let config = ReplicaSyncCoordinatorConfig::default();
    let (storage, _dir) = test_storage();
    let coordinator =
        ReplicaSyncCoordinator::new(config, storage, ALICE_SS58.to_string(), Box::new(mock));

    let result = coordinator.sync_and_confirm(&duty).await;
    // The HTTP round trip with the primary succeeds (the peaks response's
    // root matches the duty's target), but `sync_from_primary` only fetches
    // peaks/subtrees today - it never calls `Storage::commit` - so the local
    // bucket's `mmr_root` stays zero and final verification fails.
    assert!(
        matches!(result, SyncResult::VerificationFailed { .. }),
        "expected VerificationFailed, got {result:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn test_stop_command() {
    let mock = MockReplicaSyncChainClient::new();
    let config = ReplicaSyncCoordinatorConfig {
        poll_interval: Duration::from_secs(60),
        ..Default::default()
    };
    let (storage, _dir) = test_storage();
    let coordinator =
        ReplicaSyncCoordinator::new(config, storage, ALICE_SS58.to_string(), Box::new(mock));

    let handle = coordinator
        .start(tokio::sync::broadcast::channel(16).1, None)
        .await
        .unwrap();
    assert!(handle.is_running());

    handle.stop().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!handle.is_running());
}

#[tokio::test(start_paused = true)]
async fn test_pause_resume() {
    let mock = MockReplicaSyncChainClient::new();
    let config = ReplicaSyncCoordinatorConfig {
        poll_interval: Duration::from_millis(50),
        ..Default::default()
    };
    let (storage, _dir) = test_storage();
    let coordinator =
        ReplicaSyncCoordinator::new(config, storage, ALICE_SS58.to_string(), Box::new(mock));

    let handle = coordinator
        .start(tokio::sync::broadcast::channel(16).1, None)
        .await
        .unwrap();

    handle.pause().await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    handle.resume().await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    handle.stop().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!handle.is_running());
}

// ─────────────────────────────────────────────────────────────────────────────
// get_active_replica_duties filter paths
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_duties_filter_insufficient_balance() {
    let agreement = ReplicaAgreementInfo {
        bucket_id: 1,
        sync_balance: 50,
        sync_price: 100,
        min_sync_interval: 0,
        last_sync: None,
    };

    let mock = MockReplicaSyncChainClient::new()
        .with_agreements(vec![agreement])
        .with_snapshot(
            1,
            BucketSnapshot {
                mmr_root: H256::repeat_byte(0xAA),
                leaf_count: 5,
            },
        );

    let config = ReplicaSyncCoordinatorConfig::default();
    let (storage, _dir) = test_storage();
    let coordinator =
        ReplicaSyncCoordinator::new(config, storage, ALICE_SS58.to_string(), Box::new(mock));

    let duties = coordinator.get_active_replica_duties().await.unwrap();
    assert!(duties.is_empty(), "insufficient balance should be filtered");
}

#[tokio::test]
async fn test_duties_filter_sync_interval_not_elapsed() {
    let agreement = ReplicaAgreementInfo {
        bucket_id: 1,
        sync_balance: 1000,
        sync_price: 100,
        min_sync_interval: 200,
        last_sync: Some((H256::repeat_byte(0xBB), 50)),
    };

    let mock = MockReplicaSyncChainClient::new()
        .with_agreements(vec![agreement])
        .with_snapshot(
            1,
            BucketSnapshot {
                mmr_root: H256::repeat_byte(0xAA),
                leaf_count: 5,
            },
        );

    let config = ReplicaSyncCoordinatorConfig::default();
    let (storage, _dir) = test_storage();
    let coordinator =
        ReplicaSyncCoordinator::new(config, storage, ALICE_SS58.to_string(), Box::new(mock));

    let duties = coordinator.get_active_replica_duties().await.unwrap();
    assert!(
        duties.is_empty(),
        "sync interval not elapsed should be filtered"
    );
}

#[tokio::test]
async fn test_duties_filter_zero_snapshot_root() {
    let agreement = ReplicaAgreementInfo {
        bucket_id: 1,
        sync_balance: 1000,
        sync_price: 100,
        min_sync_interval: 0,
        last_sync: None,
    };

    let mock = MockReplicaSyncChainClient::new()
        .with_agreements(vec![agreement])
        .with_snapshot(
            1,
            BucketSnapshot {
                mmr_root: H256::zero(),
                leaf_count: 0,
            },
        );

    let config = ReplicaSyncCoordinatorConfig::default();
    let (storage, _dir) = test_storage();
    let coordinator =
        ReplicaSyncCoordinator::new(config, storage, ALICE_SS58.to_string(), Box::new(mock));

    let duties = coordinator.get_active_replica_duties().await.unwrap();
    assert!(duties.is_empty(), "zero snapshot root should be filtered");
}

#[tokio::test]
async fn test_duties_filter_already_synced() {
    let (storage, _nonce_store, _dir) = temp_rocksdb();
    storage
        .init_bucket(1, u64::MAX)
        .expect("bucket initialises");

    let data = b"synced data".to_vec();
    let data_root = blake2_256(&data);
    storage.store_node(1, data_root, data, None).unwrap();
    let (mmr_root, _, _) = storage.commit(1, vec![data_root]).unwrap();

    let agreement = ReplicaAgreementInfo {
        bucket_id: 1,
        sync_balance: 1000,
        sync_price: 100,
        min_sync_interval: 0,
        last_sync: None,
    };

    let mock = MockReplicaSyncChainClient::new()
        .with_agreements(vec![agreement])
        .with_snapshot(
            1,
            BucketSnapshot {
                mmr_root,
                leaf_count: 1,
            },
        );

    let config = ReplicaSyncCoordinatorConfig::default();
    let coordinator =
        ReplicaSyncCoordinator::new(config, storage, ALICE_SS58.to_string(), Box::new(mock));

    let duties = coordinator.get_active_replica_duties().await.unwrap();
    assert!(duties.is_empty(), "already synced should be filtered");
}

#[tokio::test]
async fn test_duties_happy_path_returns_duty() {
    let agreement = ReplicaAgreementInfo {
        bucket_id: 42,
        sync_balance: 1000,
        sync_price: 100,
        min_sync_interval: 0,
        last_sync: None,
    };

    let target_root = H256::repeat_byte(0xCC);
    let mock = MockReplicaSyncChainClient::new()
        .with_agreements(vec![agreement])
        .with_snapshot(
            42,
            BucketSnapshot {
                mmr_root: target_root,
                leaf_count: 10,
            },
        )
        .with_endpoints(42, vec!["http://primary:3333".to_string()]);

    let config = ReplicaSyncCoordinatorConfig::default();
    let (storage, _dir) = test_storage();
    let coordinator =
        ReplicaSyncCoordinator::new(config, storage, ALICE_SS58.to_string(), Box::new(mock));

    let duties = coordinator.get_active_replica_duties().await.unwrap();
    assert_eq!(duties.len(), 1);

    let duty = &duties[0];
    assert_eq!(duty.bucket_id, 42);
    assert_eq!(duty.target_mmr_root, target_root);
    assert_eq!(duty.target_leaf_count, 10);
    assert_eq!(duty.primary_endpoints, vec!["http://primary:3333"]);
    assert_eq!(duty.sync_balance, 1000);
    assert_eq!(duty.sync_price, 100);
}

// ─────────────────────────────────────────────────────────────────────────────
// Handle commands: force_sync, status
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test(start_paused = true)]
async fn test_status_command() {
    let mock = MockReplicaSyncChainClient::new();
    let config = ReplicaSyncCoordinatorConfig {
        poll_interval: Duration::from_secs(60),
        ..Default::default()
    };
    let (storage, _dir) = test_storage();
    let coordinator =
        ReplicaSyncCoordinator::new(config, storage, ALICE_SS58.to_string(), Box::new(mock));

    let handle = coordinator
        .start(tokio::sync::broadcast::channel(16).1, None)
        .await
        .unwrap();

    let status = handle.status().await.unwrap();
    assert!(status.running);
    assert!(!status.paused);
    assert_eq!(status.active_syncs, 0);

    handle.stop().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test(start_paused = true)]
async fn test_force_sync_command() {
    let mock = MockReplicaSyncChainClient::new();
    let config = ReplicaSyncCoordinatorConfig {
        poll_interval: Duration::from_secs(60),
        ..Default::default()
    };
    let (storage, _dir) = test_storage();
    let coordinator =
        ReplicaSyncCoordinator::new(config, storage, ALICE_SS58.to_string(), Box::new(mock));

    let handle = coordinator
        .start(tokio::sync::broadcast::channel(16).1, None)
        .await
        .unwrap();

    let result = handle.force_sync(999).await;
    assert!(result.is_ok());

    handle.stop().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
}
