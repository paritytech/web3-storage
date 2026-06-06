//! Integration tests for the replica sync coordinator (moved from unit tests for CI coverage).

use sp_core::H256;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use storage_primitives::BucketId;
use storage_provider_node::replica_sync_coordinator::{BucketSnapshot, ReplicaAgreementInfo};
use storage_provider_node::{
    Error, ProviderState, ReplicaSyncChainClient, ReplicaSyncCoordinator,
    ReplicaSyncCoordinatorConfig, Storage, SyncDuty, SyncResult,
};
use tokio::sync::Mutex;

struct MockReplicaSyncChainClient {
    block: Mutex<u64>,
    agreements: Mutex<Vec<ReplicaAgreementInfo>>,
    snapshots: Mutex<HashMap<BucketId, BucketSnapshot>>,
    endpoints: Mutex<HashMap<BucketId, Vec<String>>>,
    confirmations: Mutex<Vec<BucketId>>,
    confirm_result: Mutex<Result<(u8, u128), Error>>,
}

#[allow(dead_code)]
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
        let rt = tokio::runtime::Handle::current();
        rt.block_on(async {
            self.snapshots.lock().await.insert(bucket_id, snapshot);
        });
        self
    }

    fn with_snapshot_sync(self, bucket_id: BucketId, snapshot: BucketSnapshot) -> Self {
        // Synchronous version: constructs a new Mutex with the snapshot pre-inserted.
        // Safe to call from inside an async runtime (unlike with_snapshot which uses block_on).
        let mut map = self.snapshots.into_inner();
        map.insert(bucket_id, snapshot);
        Self {
            snapshots: Mutex::new(map),
            ..self
        }
    }

    fn with_endpoints(self, bucket_id: BucketId, endpoints: Vec<String>) -> Self {
        let rt = tokio::runtime::Handle::current();
        rt.block_on(async {
            self.endpoints.lock().await.insert(bucket_id, endpoints);
        });
        self
    }

    fn with_endpoints_sync(self, bucket_id: BucketId, endpoints: Vec<String>) -> Self {
        let mut map = self.endpoints.into_inner();
        map.insert(bucket_id, endpoints);
        Self {
            endpoints: Mutex::new(map),
            ..self
        }
    }
}

impl ReplicaSyncChainClient for MockReplicaSyncChainClient {
    fn get_current_block(&self) -> Pin<Box<dyn Future<Output = Result<u64, Error>> + Send + '_>> {
        Box::pin(async { Ok(*self.block.lock().await) })
    }

    fn fetch_replica_agreements(
        &self,
        _provider_account: &str,
        _local_buckets: Vec<BucketId>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<ReplicaAgreementInfo>, Error>> + Send + '_>> {
        Box::pin(async { Ok(self.agreements.lock().await.clone()) })
    }

    fn fetch_bucket_snapshot(
        &self,
        bucket_id: BucketId,
    ) -> Pin<Box<dyn Future<Output = Result<BucketSnapshot, Error>> + Send + '_>> {
        Box::pin(async move {
            let snapshots = self.snapshots.lock().await;
            Ok(snapshots
                .get(&bucket_id)
                .cloned()
                .unwrap_or(BucketSnapshot {
                    mmr_root: H256::zero(),
                    leaf_count: 0,
                }))
        })
    }

    fn fetch_primary_endpoints(
        &self,
        bucket_id: BucketId,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, Error>> + Send + '_>> {
        Box::pin(async move {
            let endpoints = self.endpoints.lock().await;
            Ok(endpoints.get(&bucket_id).cloned().unwrap_or_default())
        })
    }

    fn submit_sync_confirmation(
        &self,
        bucket_id: BucketId,
        _target_mmr_root: H256,
    ) -> Pin<Box<dyn Future<Output = Result<(u8, u128), Error>> + Send + '_>> {
        Box::pin(async move {
            self.confirmations.lock().await.push(bucket_id);
            let result = &*self.confirm_result.lock().await;
            match result {
                Ok(v) => Ok(*v),
                Err(e) => Err(Error::Internal(e.to_string())),
            }
        })
    }
}

fn test_state() -> Arc<ProviderState> {
    let storage = Arc::new(Storage::new());
    Arc::new(ProviderState::new(
        storage,
        "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY".to_string(),
    ))
}

#[test]
fn test_config_default() {
    let config = ReplicaSyncCoordinatorConfig::default();
    assert_eq!(config.poll_interval, Duration::from_secs(12));
    assert_eq!(config.max_concurrent_syncs, 3);
    assert!(config.auto_confirm);
}

#[test]
fn test_sync_result_variants() {
    let success = SyncResult::Success {
        bucket_id: 1,
        mmr_root: H256::zero(),
        position_matched: 0,
        payment: 1000,
    };
    assert!(matches!(success, SyncResult::Success { .. }));

    let insufficient = SyncResult::InsufficientBalance {
        bucket_id: 1,
        required: 1000,
        available: 500,
    };
    assert!(matches!(
        insufficient,
        SyncResult::InsufficientBalance { .. }
    ));

    let interval = SyncResult::SyncIntervalNotElapsed {
        bucket_id: 1,
        blocks_remaining: 50,
    };
    assert!(matches!(
        interval,
        SyncResult::SyncIntervalNotElapsed { .. }
    ));
}

#[tokio::test]
async fn test_no_agreements() {
    let mock = MockReplicaSyncChainClient::new();
    let state = test_state();
    let config = ReplicaSyncCoordinatorConfig::default();
    let coordinator = ReplicaSyncCoordinator::new(config, state, Box::new(mock));

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
    let state = test_state();
    let config = ReplicaSyncCoordinatorConfig::default();
    let coordinator = ReplicaSyncCoordinator::new(config, state, Box::new(mock));

    let result = coordinator.sync_and_confirm(&duty).await;
    assert!(matches!(result, SyncResult::InsufficientBalance { .. }));
}

#[tokio::test]
async fn test_already_synced() {
    // Create storage with a bucket whose root matches the target
    let storage = Arc::new(Storage::new());
    storage.init_bucket(1, u64::MAX);
    let data = b"test data".to_vec();
    let hash = sp_core::hashing::blake2_256(&data);
    let data_root = H256::from(hash);
    let _ = storage.store_node(1, data_root, data, None);
    let (mmr_root, _, _) = storage.commit(1, vec![data_root]).unwrap();

    let state = Arc::new(ProviderState::new(storage, "test".to_string()));

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
    let coordinator = ReplicaSyncCoordinator::new(config, state, Box::new(mock));

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
    let state = test_state();
    let config = ReplicaSyncCoordinatorConfig::default();
    let coordinator = ReplicaSyncCoordinator::new(config, state, Box::new(mock));

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
    let state = test_state();
    let config = ReplicaSyncCoordinatorConfig::default();
    let coordinator = ReplicaSyncCoordinator::new(config, state, Box::new(mock));

    let result = coordinator.sync_and_confirm(&duty).await;
    assert!(matches!(result, SyncResult::PrimaryUnavailable { .. }));
}

#[tokio::test]
async fn test_stop_command() {
    let mock = MockReplicaSyncChainClient::new();
    let state = test_state();
    let config = ReplicaSyncCoordinatorConfig {
        poll_interval: Duration::from_secs(60),
        ..Default::default()
    };
    let coordinator = ReplicaSyncCoordinator::new(config, state, Box::new(mock));

    let handle = coordinator.start(None).await.unwrap();
    assert!(handle.is_running());

    handle.stop().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!handle.is_running());
}

#[tokio::test]
async fn test_pause_resume() {
    let mock = MockReplicaSyncChainClient::new();
    let state = test_state();
    let config = ReplicaSyncCoordinatorConfig {
        poll_interval: Duration::from_millis(50),
        ..Default::default()
    };
    let coordinator = ReplicaSyncCoordinator::new(config, state, Box::new(mock));

    let handle = coordinator.start(None).await.unwrap();

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
        sync_price: 100, // balance < price → should be skipped
        min_sync_interval: 0,
        last_sync: None,
    };

    let mock = MockReplicaSyncChainClient::new()
        .with_agreements(vec![agreement])
        .with_snapshot_sync(
            1,
            BucketSnapshot {
                mmr_root: H256::repeat_byte(0xAA),
                leaf_count: 5,
            },
        );

    let state = test_state();
    let config = ReplicaSyncCoordinatorConfig::default();
    let coordinator = ReplicaSyncCoordinator::new(config, state, Box::new(mock));

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
        last_sync: Some((H256::repeat_byte(0xBB), 50)), // last sync at block 50, current=100, elapsed=50 < 200
    };

    let mock = MockReplicaSyncChainClient::new()
        .with_agreements(vec![agreement])
        .with_snapshot_sync(
            1,
            BucketSnapshot {
                mmr_root: H256::repeat_byte(0xAA),
                leaf_count: 5,
            },
        );

    let state = test_state();
    let config = ReplicaSyncCoordinatorConfig::default();
    let coordinator = ReplicaSyncCoordinator::new(config, state, Box::new(mock));

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

    // Snapshot with zero root → no data to sync
    let mock = MockReplicaSyncChainClient::new()
        .with_agreements(vec![agreement])
        .with_snapshot_sync(
            1,
            BucketSnapshot {
                mmr_root: H256::zero(),
                leaf_count: 0,
            },
        );

    let state = test_state();
    let config = ReplicaSyncCoordinatorConfig::default();
    let coordinator = ReplicaSyncCoordinator::new(config, state, Box::new(mock));

    let duties = coordinator.get_active_replica_duties().await.unwrap();
    assert!(duties.is_empty(), "zero snapshot root should be filtered");
}

#[tokio::test]
async fn test_duties_filter_already_synced() {
    // Set up local storage with the same root as the snapshot
    let storage = Arc::new(Storage::new());
    storage.init_bucket(1, u64::MAX);

    let data = b"synced data".to_vec();
    let hash = sp_core::hashing::blake2_256(&data);
    let data_root = H256::from(hash);
    storage.store_node(1, data_root, data, None).unwrap();
    let (mmr_root, _, _) = storage.commit(1, vec![data_root]).unwrap();

    let state = Arc::new(ProviderState::new(
        storage,
        "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY".to_string(),
    ));

    let agreement = ReplicaAgreementInfo {
        bucket_id: 1,
        sync_balance: 1000,
        sync_price: 100,
        min_sync_interval: 0,
        last_sync: None,
    };

    let mock = MockReplicaSyncChainClient::new()
        .with_agreements(vec![agreement])
        .with_snapshot_sync(
            1,
            BucketSnapshot {
                mmr_root, // same as local
                leaf_count: 1,
            },
        );

    let config = ReplicaSyncCoordinatorConfig::default();
    let coordinator = ReplicaSyncCoordinator::new(config, state, Box::new(mock));

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
        .with_snapshot_sync(
            42,
            BucketSnapshot {
                mmr_root: target_root,
                leaf_count: 10,
            },
        )
        .with_endpoints_sync(42, vec!["http://primary:3333".to_string()]);

    let state = test_state();
    let config = ReplicaSyncCoordinatorConfig::default();
    let coordinator = ReplicaSyncCoordinator::new(config, state, Box::new(mock));

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

#[tokio::test]
async fn test_status_command() {
    let mock = MockReplicaSyncChainClient::new();
    let state = test_state();
    let config = ReplicaSyncCoordinatorConfig {
        poll_interval: Duration::from_secs(60),
        ..Default::default()
    };
    let coordinator = ReplicaSyncCoordinator::new(config, state, Box::new(mock));

    let handle = coordinator.start(None).await.unwrap();

    let status = handle.status().await.unwrap();
    assert!(status.running);
    assert!(!status.paused);
    assert_eq!(status.active_syncs, 0);

    handle.stop().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test]
async fn test_force_sync_command() {
    let mock = MockReplicaSyncChainClient::new();
    let state = test_state();
    let config = ReplicaSyncCoordinatorConfig {
        poll_interval: Duration::from_secs(60),
        ..Default::default()
    };
    let coordinator = ReplicaSyncCoordinator::new(config, state, Box::new(mock));

    let handle = coordinator.start(None).await.unwrap();

    // force_sync should succeed (sends command, doesn't wait for completion)
    let result = handle.force_sync(999).await;
    assert!(result.is_ok());

    handle.stop().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
}
