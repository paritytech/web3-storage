// SPDX-License-Identifier: Apache-2.0

//! Integration tests for the replica sync coordinator.

mod common;

use common::{hex_hash, peaks_body, spawn_primary, test_storage};
use provider_replica::coordinator::{BucketSnapshot, ReplicaAgreementInfo};
use provider_replica::{
    ChainClientError, Error, ReplicaSyncChainClient, ReplicaSyncCoordinator,
    ReplicaSyncCoordinatorConfig, SignedSyncRoots, SyncDuty, SyncResult, SyncRoots,
    SyncRootsSigner,
};
use provider_storage::StorageBackend;
use sp_core::H256;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use storage_primitives::{blake2_256, BucketId};

/// Full Alice SS58 address (substrate prefix 42).
const ALICE_SS58: &str = "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY";

/// Stand-in for the node's scheme-tagged keypair: signs with sr25519 `//Alice`,
/// the account `ALICE_SS58` names, so attestations verify under it.
struct AliceSigner(sp_core::sr25519::Pair);

impl AliceSigner {
    fn new() -> Arc<Self> {
        use sp_core::Pair as _;
        Arc::new(Self(
            sp_core::sr25519::Pair::from_string("//Alice", None).unwrap(),
        ))
    }
}

impl SyncRootsSigner for AliceSigner {
    fn sign_sync_roots(
        &self,
        roots: &SyncRoots,
    ) -> Result<sp_runtime::MultiSignature, provider_types::SigningRefused> {
        use codec::Encode;
        use sp_core::Pair as _;
        Ok(sp_runtime::MultiSignature::Sr25519(
            self.0.sign(&roots.encode()),
        ))
    }
}

/// The coordinator under test, registered as [`ALICE_SS58`] and without a
/// signer: it syncs data but refuses to submit confirmations.
fn coordinator(
    config: ReplicaSyncCoordinatorConfig,
    storage: Arc<dyn StorageBackend>,
    chain_client: impl ReplicaSyncChainClient + 'static,
) -> ReplicaSyncCoordinator {
    coordinator_with_signer(config, storage, chain_client, None)
}

/// The same coordinator with an explicit signer, for the confirm path.
fn coordinator_with_signer(
    config: ReplicaSyncCoordinatorConfig,
    storage: Arc<dyn StorageBackend>,
    chain_client: impl ReplicaSyncChainClient + 'static,
    signer: Option<Arc<dyn SyncRootsSigner>>,
) -> ReplicaSyncCoordinator {
    ReplicaSyncCoordinator::new(
        config,
        storage,
        ALICE_SS58.to_string(),
        Box::new(chain_client),
        signer,
    )
}

struct MockReplicaSyncChainClient {
    block: Mutex<u64>,
    agreements: Mutex<Vec<ReplicaAgreementInfo>>,
    snapshots: Mutex<HashMap<BucketId, BucketSnapshot>>,
    endpoints: Mutex<HashMap<BucketId, Vec<String>>>,
    confirmations: Mutex<Vec<BucketId>>,
    attestations: Mutex<Vec<SignedSyncRoots>>,
    confirm_result: Mutex<Result<(u8, u128), ChainClientError>>,
    replica_endpoints: Mutex<Result<Vec<String>, ChainClientError>>,
}

impl MockReplicaSyncChainClient {
    fn new() -> Self {
        Self {
            block: Mutex::new(100),
            agreements: Mutex::new(Vec::new()),
            snapshots: Mutex::new(HashMap::new()),
            endpoints: Mutex::new(HashMap::new()),
            confirmations: Mutex::new(Vec::new()),
            attestations: Mutex::new(Vec::new()),
            confirm_result: Mutex::new(Ok((0, 1000))),
            replica_endpoints: Mutex::new(Ok(Vec::new())),
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

    fn with_replica_endpoints(self, result: Result<Vec<String>, ChainClientError>) -> Self {
        Self {
            replica_endpoints: Mutex::new(result),
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
    async fn get_current_block(&self) -> Result<u64, ChainClientError> {
        Ok(*self.block.lock().unwrap())
    }

    async fn fetch_replica_agreements(
        &self,
        _provider_account: &str,
        _local_buckets: Vec<BucketId>,
    ) -> Result<Vec<ReplicaAgreementInfo>, ChainClientError> {
        Ok(self.agreements.lock().unwrap().clone())
    }

    async fn fetch_bucket_snapshot(
        &self,
        bucket_id: BucketId,
    ) -> Result<BucketSnapshot, ChainClientError> {
        let snapshots = self.snapshots.lock().unwrap();
        Ok(snapshots
            .get(&bucket_id)
            .cloned()
            .unwrap_or(BucketSnapshot {
                mmr_root: H256::zero(),
                leaf_count: 0,
            }))
    }

    async fn fetch_primary_endpoints(
        &self,
        bucket_id: BucketId,
    ) -> Result<Vec<String>, ChainClientError> {
        let endpoints = self.endpoints.lock().unwrap();
        Ok(endpoints.get(&bucket_id).cloned().unwrap_or_default())
    }

    async fn fetch_replica_endpoints(
        &self,
        _bucket_id: BucketId,
    ) -> Result<Vec<String>, ChainClientError> {
        self.replica_endpoints
            .lock()
            .unwrap()
            .as_ref()
            .cloned()
            .map_err(|e| ChainClientError::query("bucket agreements", e))
    }

    async fn submit_sync_confirmation(
        &self,
        bucket_id: BucketId,
        attestation: SignedSyncRoots,
    ) -> Result<(u8, u128), ChainClientError> {
        self.confirmations.lock().unwrap().push(bucket_id);
        self.attestations.lock().unwrap().push(attestation);
        let result = &*self.confirm_result.lock().unwrap();
        match result {
            Ok(v) => Ok(*v),
            Err(e) => Err(ChainClientError::tx_rejected("confirm_replica_sync", e)),
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
    let coordinator = coordinator(config, storage, mock);

    let duties = coordinator.get_active_replica_duties().await.unwrap();
    assert!(duties.is_empty());
}

#[tokio::test]
async fn confirm_on_chain_attests_roots_with_signing_key() {
    use codec::Encode;
    use sp_core::Pair as _;
    use sp_runtime::traits::Verify;

    let target = H256::repeat_byte(0xAB);
    let duty = SyncDuty {
        bucket_id: 42,
        target_mmr_root: target,
        target_leaf_count: 10,
        source_endpoints: vec![],
        sync_balance: 1_000,
        sync_price: 100,
        min_sync_interval: 0,
        last_sync: None,
    };

    let mock = Arc::new(MockReplicaSyncChainClient::new());
    let (storage, _dir) = test_storage();
    let config = ReplicaSyncCoordinatorConfig::default();
    let coordinator =
        coordinator_with_signer(config, storage, mock.clone(), Some(AliceSigner::new()));

    let result = coordinator.confirm_on_chain(&duty).await;
    assert!(matches!(result, SyncResult::Success { bucket_id: 42, .. }));

    // The submitted roots have the target at position 0 (rest empty) and
    // the signature verifies over their SCALE encoding under //Alice —
    // exactly what the pallet checks against the registered public_key.
    let attestation = mock.attestations.lock().unwrap()[0].clone();
    let mut expected_roots = [None; 7];
    expected_roots[0] = Some(target);
    assert_eq!(attestation.roots, expected_roots);
    let alice = sp_core::sr25519::Pair::from_string("//Alice", None).unwrap();
    let expected_signer = sp_runtime::AccountId32::new(sp_core::Pair::public(&alice).0);
    assert!(
        attestation
            .signature
            .verify(&attestation.roots.encode()[..], &expected_signer),
        "attestation must verify under //Alice's key"
    );
    assert_eq!(mock.confirmations.lock().unwrap().as_slice(), &[42]);
}

#[tokio::test]
async fn confirm_on_chain_surfaces_submission_errors() {
    let duty = SyncDuty {
        bucket_id: 9,
        target_mmr_root: H256::repeat_byte(0xEF),
        target_leaf_count: 1,
        source_endpoints: vec![],
        sync_balance: 1_000,
        sync_price: 100,
        min_sync_interval: 0,
        last_sync: None,
    };

    let mock = Arc::new(MockReplicaSyncChainClient::new());
    *mock.confirm_result.lock().unwrap() = Err(ChainClientError::tx_rejected(
        "confirm_replica_sync",
        "chain rejected",
    ));
    let (storage, _dir) = test_storage();
    let config = ReplicaSyncCoordinatorConfig::default();
    let coordinator =
        coordinator_with_signer(config, storage, mock.clone(), Some(AliceSigner::new()));

    let result = coordinator.confirm_on_chain(&duty).await;
    match result {
        SyncResult::SubmissionFailed { bucket_id, error } => {
            assert_eq!(bucket_id, 9);
            assert!(
                error.contains("chain rejected"),
                "unexpected error: {error}"
            );
        }
        other => panic!("expected SubmissionFailed, got {other:?}"),
    }
}

#[tokio::test]
async fn confirm_on_chain_refuses_without_signing_key() {
    let duty = SyncDuty {
        bucket_id: 7,
        target_mmr_root: H256::repeat_byte(0xCD),
        target_leaf_count: 1,
        source_endpoints: vec![],
        sync_balance: 1_000,
        sync_price: 100,
        min_sync_interval: 0,
        last_sync: None,
    };

    let mock = Arc::new(MockReplicaSyncChainClient::new());
    // Provider-id mode: no signer attached.
    let (storage, _dir) = test_storage();
    let config = ReplicaSyncCoordinatorConfig::default();
    let coordinator = coordinator(config, storage, mock.clone());

    let result = coordinator.confirm_on_chain(&duty).await;
    match result {
        SyncResult::SubmissionFailed { bucket_id, error } => {
            assert_eq!(bucket_id, 7);
            assert!(
                error.contains("no signing key"),
                "unexpected error: {error}"
            );
        }
        other => panic!("expected SubmissionFailed, got {other:?}"),
    }
    // Nothing must reach the chain without an attestation.
    assert!(mock.confirmations.lock().unwrap().is_empty());
}

#[tokio::test]
async fn test_insufficient_balance() {
    let duty = SyncDuty {
        bucket_id: 1,
        target_mmr_root: H256::repeat_byte(0xAA),
        target_leaf_count: 10,
        source_endpoints: vec![],
        sync_balance: 50,
        sync_price: 100,
        min_sync_interval: 0,
        last_sync: None,
    };

    let mock = MockReplicaSyncChainClient::new();
    let config = ReplicaSyncCoordinatorConfig::default();
    let (storage, _dir) = test_storage();
    let coordinator = coordinator(config, storage, mock);

    let result = coordinator.sync_and_confirm(&duty).await;
    assert!(matches!(result, SyncResult::InsufficientBalance { .. }));
}

#[tokio::test]
async fn test_already_synced() {
    let (storage, _dir) = test_storage();
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
        source_endpoints: vec![],
        sync_balance: 1000,
        sync_price: 100,
        min_sync_interval: 0,
        last_sync: None,
    };

    let mock = MockReplicaSyncChainClient::new();
    let config = ReplicaSyncCoordinatorConfig::default();
    let coordinator =
        ReplicaSyncCoordinator::new(config, storage, "test".to_string(), Box::new(mock), None);

    let result = coordinator.sync_and_confirm(&duty).await;
    assert!(matches!(result, SyncResult::AlreadySynced { .. }));
}

#[tokio::test]
async fn test_no_data_to_sync() {
    let duty = SyncDuty {
        bucket_id: 1,
        target_mmr_root: H256::zero(),
        target_leaf_count: 0,
        source_endpoints: vec![],
        sync_balance: 1000,
        sync_price: 100,
        min_sync_interval: 0,
        last_sync: None,
    };

    let mock = MockReplicaSyncChainClient::new();
    let config = ReplicaSyncCoordinatorConfig::default();
    let (storage, _dir) = test_storage();
    let coordinator = coordinator(config, storage, mock);

    let result = coordinator.sync_and_confirm(&duty).await;
    assert!(matches!(result, SyncResult::NoDataToSync { .. }));
}

#[tokio::test]
async fn test_primary_unavailable() {
    let duty = SyncDuty {
        bucket_id: 1,
        target_mmr_root: H256::repeat_byte(0xAA),
        target_leaf_count: 10,
        source_endpoints: vec!["http://127.0.0.1:19999".to_string()],
        sync_balance: 1000,
        sync_price: 100,
        min_sync_interval: 0,
        last_sync: None,
    };

    let mock = MockReplicaSyncChainClient::new();
    let config = ReplicaSyncCoordinatorConfig::default();
    let (storage, _dir) = test_storage();
    let coordinator = coordinator(config, storage, mock);

    let result = coordinator.sync_and_confirm(&duty).await;
    assert!(matches!(result, SyncResult::SourcesUnavailable { .. }));
}

#[tokio::test]
async fn test_sync_from_source_succeeds_but_final_verification_fails() {
    let bucket_id = 1;
    let target_root = H256::repeat_byte(0xDD);
    let primary_url = spawn_primary(peaks_body(&hex_hash(target_root), &[])).await;

    let duty = SyncDuty {
        bucket_id,
        target_mmr_root: target_root,
        target_leaf_count: 0,
        source_endpoints: vec![primary_url],
        sync_balance: 1000,
        sync_price: 100,
        min_sync_interval: 0,
        last_sync: None,
    };

    let mock = MockReplicaSyncChainClient::new();
    let config = ReplicaSyncCoordinatorConfig::default();
    let (storage, _dir) = test_storage();
    let coordinator = coordinator(config, storage, mock);

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
    let coordinator = coordinator(config, storage, mock);

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
async fn test_command_after_stop_yields_channel_closed() {
    let mock = MockReplicaSyncChainClient::new();
    let config = ReplicaSyncCoordinatorConfig {
        poll_interval: Duration::from_secs(60),
        ..Default::default()
    };
    let (storage, _dir) = test_storage();
    let coordinator = coordinator(config, storage, mock);

    let handle = coordinator
        .start(tokio::sync::broadcast::channel(16).1, None)
        .await
        .unwrap();

    // Stopping ends the coordinator's background task, which drops its
    // command channel receiver. Any command sent afterward can no longer be
    // delivered.
    handle.stop().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    assert!(matches!(handle.pause().await, Err(Error::ChannelClosed)));
}

#[tokio::test(start_paused = true)]
async fn test_pause_resume() {
    let mock = MockReplicaSyncChainClient::new();
    let config = ReplicaSyncCoordinatorConfig {
        poll_interval: Duration::from_millis(50),
        ..Default::default()
    };
    let (storage, _dir) = test_storage();
    let coordinator = coordinator(config, storage, mock);

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
    let coordinator = coordinator(config, storage, mock);

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
    let coordinator = coordinator(config, storage, mock);

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
    let coordinator = coordinator(config, storage, mock);

    let duties = coordinator.get_active_replica_duties().await.unwrap();
    assert!(duties.is_empty(), "zero snapshot root should be filtered");
}

#[tokio::test]
async fn test_duties_filter_already_synced() {
    let (storage, _dir) = test_storage();
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
    let coordinator = coordinator(config, storage, mock);

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
    let coordinator = coordinator(config, storage, mock);

    let duties = coordinator.get_active_replica_duties().await.unwrap();
    assert_eq!(duties.len(), 1);

    let duty = &duties[0];
    assert_eq!(duty.bucket_id, 42);
    assert_eq!(duty.target_mmr_root, target_root);
    assert_eq!(duty.target_leaf_count, 10);
    assert_eq!(duty.source_endpoints, vec!["http://primary:3333"]);
    assert_eq!(duty.sync_balance, 1000);
    assert_eq!(duty.sync_price, 100);
}

#[tokio::test]
async fn test_duty_sources_append_replicas_after_primaries_deduped() {
    // Primaries come first (most current); the bucket's other replicas follow
    // as the fallback for private buckets, and an endpoint that is both never
    // appears twice.
    let agreement = ReplicaAgreementInfo {
        bucket_id: 42,
        sync_balance: 1000,
        sync_price: 100,
        min_sync_interval: 0,
        last_sync: None,
    };
    let mock = MockReplicaSyncChainClient::new()
        .with_agreements(vec![agreement])
        .with_snapshot(
            42,
            BucketSnapshot {
                mmr_root: H256::repeat_byte(0xCC),
                leaf_count: 10,
            },
        )
        .with_endpoints(42, vec!["http://primary:3333".to_string()])
        .with_replica_endpoints(Ok(vec![
            "http://primary:3333".to_string(),
            "http://replica:3334".to_string(),
        ]));

    let (storage, _dir) = test_storage();
    let coordinator = coordinator(ReplicaSyncCoordinatorConfig::default(), storage, mock);

    let duties = coordinator.get_active_replica_duties().await.unwrap();
    assert_eq!(
        duties[0].source_endpoints,
        vec!["http://primary:3333", "http://replica:3334"]
    );
}

#[tokio::test]
async fn test_duty_sources_degrade_to_primaries_when_replica_listing_fails() {
    // Listing replicas is best-effort: a failure only shrinks the fallback
    // set, it must not sink the whole duty.
    let agreement = ReplicaAgreementInfo {
        bucket_id: 42,
        sync_balance: 1000,
        sync_price: 100,
        min_sync_interval: 0,
        last_sync: None,
    };
    let mock = MockReplicaSyncChainClient::new()
        .with_agreements(vec![agreement])
        .with_snapshot(
            42,
            BucketSnapshot {
                mmr_root: H256::repeat_byte(0xCC),
                leaf_count: 10,
            },
        )
        .with_endpoints(42, vec!["http://primary:3333".to_string()])
        .with_replica_endpoints(Err(ChainClientError::query(
            "bucket agreements",
            "chain down",
        )));

    let (storage, _dir) = test_storage();
    let coordinator = coordinator(ReplicaSyncCoordinatorConfig::default(), storage, mock);

    let duties = coordinator.get_active_replica_duties().await.unwrap();
    assert_eq!(duties[0].source_endpoints, vec!["http://primary:3333"]);
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
    let coordinator = coordinator(config, storage, mock);

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
    let coordinator = coordinator(config, storage, mock);

    let handle = coordinator
        .start(tokio::sync::broadcast::channel(16).1, None)
        .await
        .unwrap();

    let result = handle.force_sync(999).await;
    assert!(result.is_ok());

    handle.stop().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
}
