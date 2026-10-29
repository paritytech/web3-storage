// SPDX-License-Identifier: Apache-2.0

//! Integration tests for the replica sync coordinator.

mod common;

use common::{source_bucket, spawn_node_source, spawn_slow, NodeSourceConfig};
use provider_events::BlockEvent;
use provider_replica::coordinator::{BucketCommitments, ReplicaAgreementInfo, SyncingBuckets};
use provider_replica::target::SyncTarget;
use provider_replica::{
    ChainClientError, Error, ReplicaSyncChainClient, ReplicaSyncCoordinator,
    ReplicaSyncCoordinatorConfig, SignedSyncRoots, SyncDuty, SyncResult, SyncRoots,
    SyncRootsSigner,
};
use provider_storage::{temp_rocksdb, ChunkTreeNode, StorageBackend};
use sp_core::H256;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use storage_primitives::{hash_leaf, BucketId, Commitment};

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
    snapshots: Mutex<HashMap<BucketId, BucketCommitments>>,
    failing_commitments: Mutex<HashSet<BucketId>>,
    commitment_reads: Mutex<Vec<BucketId>>,
    endpoints: Mutex<HashMap<BucketId, Vec<String>>>,
    confirmations: Mutex<Vec<BucketId>>,
    attestations: Mutex<Vec<SignedSyncRoots>>,
    confirm_result: Mutex<Result<Option<(u8, u128)>, ChainClientError>>,
    replica_endpoints: Mutex<Result<Vec<String>, ChainClientError>>,
}

impl MockReplicaSyncChainClient {
    fn new() -> Self {
        Self {
            block: Mutex::new(100),
            agreements: Mutex::new(Vec::new()),
            snapshots: Mutex::new(HashMap::new()),
            failing_commitments: Mutex::new(HashSet::new()),
            commitment_reads: Mutex::new(Vec::new()),
            endpoints: Mutex::new(HashMap::new()),
            confirmations: Mutex::new(Vec::new()),
            attestations: Mutex::new(Vec::new()),
            confirm_result: Mutex::new(Ok(Some((0, 1000)))),
            replica_endpoints: Mutex::new(Ok(Vec::new())),
        }
    }

    fn with_agreements(self, agreements: Vec<ReplicaAgreementInfo>) -> Self {
        Self {
            agreements: Mutex::new(agreements),
            ..self
        }
    }

    fn with_snapshot(self, bucket_id: BucketId, snapshot: BucketCommitments) -> Self {
        let mut map = self.snapshots.into_inner().unwrap();
        map.insert(bucket_id, snapshot);
        Self {
            snapshots: Mutex::new(map),
            ..self
        }
    }

    /// The chain's snapshot of `bucket_id` has the root `mmr_root`.
    fn with_snapshot_root(self, bucket_id: BucketId, mmr_root: H256) -> Self {
        self.with_snapshot(
            bucket_id,
            BucketCommitments {
                snapshot: Commitment {
                    mmr_root,
                    start_seq: 0,
                    leaf_count: 1,
                },
                ..Default::default()
            },
        )
    }

    /// Reading the commitments of `bucket_id` fails.
    fn with_failing_commitments(self, bucket_id: BucketId) -> Self {
        self.failing_commitments.lock().unwrap().insert(bucket_id);
        self
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

    async fn get_anchor_block_time(&self) -> Result<Duration, ChainClientError> {
        Ok(Duration::from_secs(6))
    }

    async fn fetch_replica_agreements(
        &self,
        _provider_account: &str,
    ) -> Result<Vec<ReplicaAgreementInfo>, ChainClientError> {
        Ok(self.agreements.lock().unwrap().clone())
    }

    async fn fetch_bucket_commitments(
        &self,
        bucket_id: BucketId,
    ) -> Result<BucketCommitments, ChainClientError> {
        self.commitment_reads.lock().unwrap().push(bucket_id);
        if self
            .failing_commitments
            .lock()
            .unwrap()
            .contains(&bucket_id)
        {
            return Err(ChainClientError::query("buckets", "unavailable"));
        }
        let snapshots = self.snapshots.lock().unwrap();
        Ok(snapshots
            .get(&bucket_id)
            .cloned()
            .unwrap_or(BucketCommitments {
                snapshot: Commitment {
                    mmr_root: H256::zero(),
                    start_seq: 0,
                    leaf_count: 0,
                },
                ..Default::default()
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
    ) -> Result<Option<(u8, u128)>, ChainClientError> {
        self.confirmations.lock().unwrap().push(bucket_id);
        self.attestations.lock().unwrap().push(attestation);
        let result = &*self.confirm_result.lock().unwrap();
        match result {
            Ok(v) => Ok(*v),
            Err(e) => Err(ChainClientError::tx_rejected("confirm_replica_sync", e)),
        }
    }
}

/// A target at position 0 (the snapshot) of `leaf_count` leaves from seq 0.
fn snapshot_target(mmr_root: H256, leaf_count: u64) -> SyncTarget {
    SyncTarget {
        position: 0,
        commitment: Commitment {
            mmr_root,
            start_seq: 0,
            leaf_count,
        },
    }
}

fn mock_duty(bucket_id: BucketId) -> SyncDuty {
    SyncDuty {
        bucket_id,
        target: snapshot_target(H256::repeat_byte(0xAB), 10),
        max_bytes: 1024 * 1024,
        source_endpoints: vec![],
        sync_balance: 1_000,
        sync_price: 100,
        min_sync_interval: 0,
        last_sync: None,
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
    let (storage, _dir) = temp_rocksdb();
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
        target: snapshot_target(target, 10),
        max_bytes: 1024 * 1024,
        source_endpoints: vec![],
        sync_balance: 1_000,
        sync_price: 100,
        min_sync_interval: 0,
        last_sync: None,
    };

    let mock = Arc::new(MockReplicaSyncChainClient::new().with_snapshot_root(42, target));
    let (storage, _dir) = temp_rocksdb();
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
async fn confirm_on_chain_reports_the_matched_position_and_payment() {
    let mock =
        Arc::new(MockReplicaSyncChainClient::new().with_snapshot_root(42, H256::repeat_byte(0xAB)));
    *mock.confirm_result.lock().unwrap() = Ok(Some((3, 250)));
    let (storage, _dir) = temp_rocksdb();
    let coordinator = coordinator_with_signer(
        ReplicaSyncCoordinatorConfig::default(),
        storage,
        mock,
        Some(AliceSigner::new()),
    );

    let result = coordinator.confirm_on_chain(&mock_duty(42)).await;
    assert!(matches!(
        result,
        SyncResult::Success {
            bucket_id: 42,
            position_matched: 3,
            payment: 250,
            ..
        }
    ));
}

#[tokio::test]
async fn confirm_on_chain_signs_the_position_the_chain_holds_the_root_at_now() {
    // The duty targets the snapshot (position 0). A checkpoint since then
    // moved the root out of the snapshot; slot 3 still holds it.
    let root = H256::repeat_byte(0xAB);
    let mut commitments = BucketCommitments {
        snapshot: Commitment {
            mmr_root: H256::repeat_byte(0xCD),
            start_seq: 0,
            leaf_count: 2,
        },
        ..Default::default()
    };
    commitments.historical_roots[3] = (
        7,
        Commitment {
            mmr_root: root,
            start_seq: 0,
            leaf_count: 1,
        },
    );
    let mock = Arc::new(MockReplicaSyncChainClient::new().with_snapshot(42, commitments));
    let (storage, _dir) = temp_rocksdb();
    let coordinator = coordinator_with_signer(
        ReplicaSyncCoordinatorConfig::default(),
        storage,
        mock.clone(),
        Some(AliceSigner::new()),
    );
    let duty = mock_duty(42);
    assert_eq!(duty.target.position, 0);

    coordinator.confirm_on_chain(&duty).await;

    let mut expected_roots = [None; 7];
    expected_roots[4] = Some(root);
    assert_eq!(mock.attestations.lock().unwrap()[0].roots, expected_roots);
}

#[tokio::test]
async fn confirm_on_chain_treats_an_earlier_confirmation_as_already_synced() {
    let mock =
        Arc::new(MockReplicaSyncChainClient::new().with_snapshot_root(42, H256::repeat_byte(0xAB)));
    *mock.confirm_result.lock().unwrap() = Ok(None);
    let (storage, _dir) = temp_rocksdb();
    let coordinator = coordinator_with_signer(
        ReplicaSyncCoordinatorConfig::default(),
        storage,
        mock,
        Some(AliceSigner::new()),
    );

    let result = coordinator.confirm_on_chain(&mock_duty(42)).await;
    assert!(matches!(
        result,
        SyncResult::AlreadySynced { bucket_id: 42, .. }
    ));
}

#[tokio::test]
async fn confirm_on_chain_surfaces_submission_errors() {
    let duty = SyncDuty {
        bucket_id: 9,
        target: snapshot_target(H256::repeat_byte(0xEF), 1),
        max_bytes: 1024 * 1024,
        source_endpoints: vec![],
        sync_balance: 1_000,
        sync_price: 100,
        min_sync_interval: 0,
        last_sync: None,
    };

    let mock =
        Arc::new(MockReplicaSyncChainClient::new().with_snapshot_root(9, H256::repeat_byte(0xEF)));
    *mock.confirm_result.lock().unwrap() = Err(ChainClientError::tx_rejected(
        "confirm_replica_sync",
        "chain rejected",
    ));
    let (storage, _dir) = temp_rocksdb();
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
        target: snapshot_target(H256::repeat_byte(0xCD), 1),
        max_bytes: 1024 * 1024,
        source_endpoints: vec![],
        sync_balance: 1_000,
        sync_price: 100,
        min_sync_interval: 0,
        last_sync: None,
    };

    let mock = Arc::new(MockReplicaSyncChainClient::new());
    // Provider-id mode: no signer attached.
    let (storage, _dir) = temp_rocksdb();
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
        target: snapshot_target(H256::repeat_byte(0xAA), 10),
        max_bytes: 1024 * 1024,
        source_endpoints: vec![],
        sync_balance: 50,
        sync_price: 100,
        min_sync_interval: 0,
        last_sync: None,
    };

    let mock = MockReplicaSyncChainClient::new();
    let config = ReplicaSyncCoordinatorConfig::default();
    let (storage, _dir) = temp_rocksdb();
    let coordinator = coordinator(config, storage, mock);

    let result = coordinator.sync_and_confirm(&duty).await;
    assert!(matches!(result, SyncResult::InsufficientBalance { .. }));
}

#[tokio::test]
async fn a_synced_root_that_is_not_confirmed_yet_is_confirmed_without_fetching() {
    let (storage, _dir) = temp_rocksdb();
    storage
        .init_bucket(1, u64::MAX)
        .expect("bucket initialises");
    let data = b"test data".to_vec();
    let data_root = hash_leaf(&data);
    let _ = storage.store_node(1, data_root, ChunkTreeNode::Chunk(data));
    let mmr_root = storage.commit(1, vec![data_root]).unwrap().mmr_root;

    // No source: everything the target needs is stored.
    let duty = SyncDuty {
        bucket_id: 1,
        target: snapshot_target(mmr_root, 1),
        max_bytes: 1024 * 1024,
        source_endpoints: vec![],
        sync_balance: 1000,
        sync_price: 100,
        min_sync_interval: 0,
        last_sync: None,
    };

    let mock = Arc::new(MockReplicaSyncChainClient::new().with_snapshot_root(1, mmr_root));
    let coordinator = coordinator_with_signer(
        ReplicaSyncCoordinatorConfig::default(),
        storage,
        mock.clone(),
        Some(AliceSigner::new()),
    );

    let result = coordinator.sync_and_confirm(&duty).await;
    assert!(matches!(result, SyncResult::Success { .. }), "{result:?}");
    assert_eq!(mock.confirmations.lock().unwrap().as_slice(), &[1]);
}

#[tokio::test]
async fn test_no_data_to_sync() {
    let duty = SyncDuty {
        bucket_id: 1,
        target: snapshot_target(H256::zero(), 0),
        max_bytes: 1024 * 1024,
        source_endpoints: vec![],
        sync_balance: 1000,
        sync_price: 100,
        min_sync_interval: 0,
        last_sync: None,
    };

    let mock = MockReplicaSyncChainClient::new();
    let config = ReplicaSyncCoordinatorConfig::default();
    let (storage, _dir) = temp_rocksdb();
    let coordinator = coordinator(config, storage, mock);

    let result = coordinator.sync_and_confirm(&duty).await;
    assert!(matches!(result, SyncResult::NoDataToSync { .. }));
}

/// A duty for bucket 1 to the commitment `target`, served by `source`.
fn duty_from(source: &str, target: SyncTarget, max_bytes: u64) -> SyncDuty {
    SyncDuty {
        bucket_id: 1,
        target,
        max_bytes,
        source_endpoints: vec![source.to_string()],
        sync_balance: 1000,
        sync_price: 100,
        min_sync_interval: 0,
        last_sync: None,
    }
}

#[tokio::test]
async fn a_fresh_replica_syncs_and_confirms_at_the_target_position() {
    let (source, _source_dir, commitment) = source_bucket(&[2, 3]);
    let node = spawn_node_source(source, NodeSourceConfig::default()).await;
    let duty = duty_from(
        &node.url,
        SyncTarget {
            position: 2,
            commitment,
        },
        u64::MAX,
    );

    let mut commitments = BucketCommitments::default();
    commitments.historical_roots[1] = (4, commitment);
    let mock = Arc::new(MockReplicaSyncChainClient::new().with_snapshot(1, commitments));
    *mock.confirm_result.lock().unwrap() = Ok(Some((2, 1000)));
    // No local bucket: the sync creates it.
    let (storage, _dir) = temp_rocksdb();
    assert!(storage.get_bucket(1).is_none());
    let coordinator = coordinator_with_signer(
        ReplicaSyncCoordinatorConfig::default(),
        Arc::clone(&storage),
        mock.clone(),
        Some(AliceSigner::new()),
    );

    let result = coordinator.sync_and_confirm(&duty).await;

    assert!(
        matches!(
            result,
            SyncResult::Success {
                bucket_id: 1,
                position_matched: 2,
                payment: 1000,
                ..
            }
        ),
        "{result:?}"
    );
    let local = storage.get_bucket(1).unwrap();
    assert_eq!(local.mmr_root, commitment.mmr_root);
    assert_eq!(local.leaf_count, 2);
    // The root is attested at the target's position and nowhere else.
    let mut expected_roots = [None; 7];
    expected_roots[2] = Some(commitment.mmr_root);
    assert_eq!(mock.attestations.lock().unwrap()[0].roots, expected_roots);
}

#[tokio::test]
async fn the_agreement_quota_limits_what_a_sync_stores() {
    let (source, _source_dir, commitment) = source_bucket(&[3]);
    let node = spawn_node_source(source, NodeSourceConfig::default()).await;
    // The bucket needs more than 100 bytes.
    let duty = duty_from(
        &node.url,
        SyncTarget {
            position: 0,
            commitment,
        },
        100,
    );

    let mock = Arc::new(MockReplicaSyncChainClient::new());
    let (storage, _dir) = temp_rocksdb();
    // A bucket created earlier without a quota gets the agreement's.
    storage.init_bucket(1, u64::MAX).unwrap();
    let coordinator = coordinator_with_signer(
        ReplicaSyncCoordinatorConfig::default(),
        Arc::clone(&storage),
        mock.clone(),
        Some(AliceSigner::new()),
    );

    let result = coordinator.sync_and_confirm(&duty).await;

    match result {
        SyncResult::StorageFailed { error, .. } => {
            assert!(error.contains("Quota exceeded"), "{error}")
        }
        other => panic!("expected StorageFailed, got {other:?}"),
    }
    assert_eq!(storage.get_bucket(1).unwrap().leaf_count, 0);
    assert!(mock.confirmations.lock().unwrap().is_empty());
}

#[tokio::test]
async fn test_primary_unavailable() {
    let duty = SyncDuty {
        bucket_id: 1,
        target: snapshot_target(H256::repeat_byte(0xAA), 10),
        max_bytes: 1024 * 1024,
        source_endpoints: vec!["http://127.0.0.1:19999".to_string()],
        sync_balance: 1000,
        sync_price: 100,
        min_sync_interval: 0,
        last_sync: None,
    };

    let mock = MockReplicaSyncChainClient::new();
    let config = ReplicaSyncCoordinatorConfig::default();
    let (storage, _dir) = temp_rocksdb();
    let coordinator = coordinator(config, storage, mock);

    let result = coordinator.sync_and_confirm(&duty).await;
    assert!(matches!(result, SyncResult::SourcesUnavailable { .. }));
}

#[tokio::test(start_paused = true)]
async fn test_stop_command() {
    let mock = MockReplicaSyncChainClient::new();
    let config = ReplicaSyncCoordinatorConfig {
        poll_interval: Duration::from_secs(60),
        ..Default::default()
    };
    let (storage, _dir) = temp_rocksdb();
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
    let (storage, _dir) = temp_rocksdb();
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
    let (storage, _dir) = temp_rocksdb();
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
        max_bytes: 1024 * 1024,
        expires_at: 10_000,
    };

    let mock = MockReplicaSyncChainClient::new()
        .with_agreements(vec![agreement])
        .with_snapshot(
            1,
            BucketCommitments {
                snapshot: Commitment {
                    mmr_root: H256::repeat_byte(0xAA),
                    start_seq: 0,
                    leaf_count: 5,
                },
                ..Default::default()
            },
        );

    let config = ReplicaSyncCoordinatorConfig::default();
    let (storage, _dir) = temp_rocksdb();
    let coordinator = coordinator(config, storage, mock);

    let duties = coordinator.get_active_replica_duties().await.unwrap();
    assert!(duties.is_empty(), "insufficient balance should be filtered");
}

/// An agreement for `bucket_id` that is due now, and a chain snapshot for it.
fn due_agreement(bucket_id: BucketId) -> ReplicaAgreementInfo {
    ReplicaAgreementInfo {
        bucket_id,
        sync_balance: 1000,
        sync_price: 100,
        min_sync_interval: 0,
        last_sync: None,
        max_bytes: 1024 * 1024,
        expires_at: 10_000,
    }
}

#[tokio::test]
async fn a_failing_agreement_does_not_hide_the_other_duties() {
    let mock = MockReplicaSyncChainClient::new()
        .with_agreements(vec![due_agreement(1), due_agreement(2)])
        .with_failing_commitments(1)
        .with_snapshot_root(2, H256::repeat_byte(0xAA));
    let (storage, _dir) = temp_rocksdb();
    let coordinator = coordinator(ReplicaSyncCoordinatorConfig::default(), storage, mock);

    let duties = coordinator.get_active_replica_duties().await.unwrap();

    let buckets: Vec<_> = duties.iter().map(|duty| duty.bucket_id).collect();
    assert_eq!(buckets, vec![2]);
}

#[tokio::test]
async fn a_syncing_bucket_gets_no_duty_and_no_chain_reads() {
    let mock = Arc::new(
        MockReplicaSyncChainClient::new()
            .with_agreements(vec![due_agreement(1), due_agreement(2)])
            .with_snapshot_root(1, H256::repeat_byte(0xAA))
            .with_snapshot_root(2, H256::repeat_byte(0xBB)),
    );
    let (storage, _dir) = temp_rocksdb();
    let syncing = SyncingBuckets::default();
    let _guard = syncing.begin(1);
    let coordinator = coordinator(
        ReplicaSyncCoordinatorConfig::default(),
        storage,
        Arc::clone(&mock),
    )
    .with_syncing_buckets(syncing);

    let duties = coordinator.get_active_replica_duties().await.unwrap();

    let buckets: Vec<_> = duties.iter().map(|duty| duty.bucket_id).collect();
    assert_eq!(buckets, vec![2]);
    assert_eq!(*mock.commitment_reads.lock().unwrap(), vec![2]);
}

#[tokio::test]
async fn test_duties_filter_sync_interval_not_elapsed() {
    let agreement = ReplicaAgreementInfo {
        bucket_id: 1,
        sync_balance: 1000,
        sync_price: 100,
        min_sync_interval: 200,
        last_sync: Some((H256::repeat_byte(0xBB), 50)),
        max_bytes: 1024 * 1024,
        expires_at: 10_000,
    };

    let mock = MockReplicaSyncChainClient::new()
        .with_agreements(vec![agreement])
        .with_snapshot(
            1,
            BucketCommitments {
                snapshot: Commitment {
                    mmr_root: H256::repeat_byte(0xAA),
                    start_seq: 0,
                    leaf_count: 5,
                },
                ..Default::default()
            },
        );

    let config = ReplicaSyncCoordinatorConfig::default();
    let (storage, _dir) = temp_rocksdb();
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
        max_bytes: 1024 * 1024,
        expires_at: 10_000,
    };

    let mock = MockReplicaSyncChainClient::new()
        .with_agreements(vec![agreement])
        .with_snapshot(
            1,
            BucketCommitments {
                snapshot: Commitment {
                    mmr_root: H256::zero(),
                    start_seq: 0,
                    leaf_count: 0,
                },
                ..Default::default()
            },
        );

    let config = ReplicaSyncCoordinatorConfig::default();
    let (storage, _dir) = temp_rocksdb();
    let coordinator = coordinator(config, storage, mock);

    let duties = coordinator.get_active_replica_duties().await.unwrap();
    assert!(duties.is_empty(), "zero snapshot root should be filtered");
}

#[tokio::test]
async fn test_duties_keep_a_stored_root_that_is_not_confirmed() {
    let (storage, _dir) = temp_rocksdb();
    storage
        .init_bucket(1, u64::MAX)
        .expect("bucket initialises");

    let data = b"synced data".to_vec();
    let data_root = hash_leaf(&data);
    storage
        .store_node(1, data_root, ChunkTreeNode::Chunk(data))
        .unwrap();
    let mmr_root = storage.commit(1, vec![data_root]).unwrap().mmr_root;

    let agreement = ReplicaAgreementInfo {
        bucket_id: 1,
        sync_balance: 1000,
        sync_price: 100,
        min_sync_interval: 0,
        last_sync: None,
        max_bytes: 1024 * 1024,
        expires_at: 10_000,
    };

    let mock = MockReplicaSyncChainClient::new()
        .with_agreements(vec![agreement])
        .with_snapshot(
            1,
            BucketCommitments {
                snapshot: Commitment {
                    mmr_root,
                    start_seq: 0,
                    leaf_count: 1,
                },
                ..Default::default()
            },
        );

    let config = ReplicaSyncCoordinatorConfig::default();
    let coordinator = coordinator(config, storage, mock);

    // The root is stored but not confirmed (`last_sync` is empty), so the
    // duty remains: the sync confirms it.
    let duties = coordinator.get_active_replica_duties().await.unwrap();
    assert_eq!(duties.len(), 1);
}

/// A chain with one due replica duty: bucket 42, target root `0xCC..`, 10
/// leaves, sync balance 1000 at price 100, one primary at
/// `http://primary:3333`.
fn mock_with_bucket_42_duty() -> MockReplicaSyncChainClient {
    MockReplicaSyncChainClient::new()
        .with_agreements(vec![ReplicaAgreementInfo {
            bucket_id: 42,
            sync_balance: 1000,
            sync_price: 100,
            min_sync_interval: 0,
            last_sync: None,
            max_bytes: 1024 * 1024,
            expires_at: 10_000,
        }])
        .with_snapshot(
            42,
            BucketCommitments {
                snapshot: Commitment {
                    mmr_root: H256::repeat_byte(0xCC),
                    start_seq: 0,
                    leaf_count: 10,
                },
                ..Default::default()
            },
        )
        .with_endpoints(42, vec!["http://primary:3333".to_string()])
}

#[tokio::test]
async fn test_duties_happy_path_returns_duty() {
    let target_root = H256::repeat_byte(0xCC);
    let mock = mock_with_bucket_42_duty();

    let config = ReplicaSyncCoordinatorConfig::default();
    let (storage, _dir) = temp_rocksdb();
    // A fresh replica has no local bucket yet and still gets the duty.
    assert!(storage.get_bucket(42).is_none());
    let coordinator = coordinator(config, storage, mock);

    let duties = coordinator.get_active_replica_duties().await.unwrap();
    assert_eq!(duties.len(), 1);

    let duty = &duties[0];
    assert_eq!(duty.bucket_id, 42);
    assert_eq!(duty.target.commitment.mmr_root, target_root);
    assert_eq!(duty.target.commitment.leaf_count, 10);
    assert_eq!(duty.target.position, 0);
    assert_eq!(duty.max_bytes, 1024 * 1024);
    assert_eq!(duty.source_endpoints, vec!["http://primary:3333"]);
    assert_eq!(duty.sync_balance, 1000);
    assert_eq!(duty.sync_price, 100);
}

#[tokio::test]
async fn test_duties_target_the_historical_slot_that_stays_valid_longest() {
    let commitment = |byte: u8| Commitment {
        mmr_root: H256::repeat_byte(byte),
        start_seq: 0,
        leaf_count: 4,
    };
    let mut commitments = BucketCommitments {
        snapshot: commitment(0xA1),
        ..Default::default()
    };
    // Slot 4 (prime 47) is overwritten at block 94, slot 1 (prime 7) at 42.
    commitments.historical_roots[1] = (5, commitment(0xB1));
    commitments.historical_roots[4] = (1, commitment(0xB4));
    let mock = mock_with_bucket_42_duty().with_snapshot(42, commitments);

    let (storage, _dir) = temp_rocksdb();
    let coordinator = coordinator(ReplicaSyncCoordinatorConfig::default(), storage, mock);

    let duties = coordinator.get_active_replica_duties().await.unwrap();

    assert_eq!(duties.len(), 1);
    assert_eq!(duties[0].target.position, 5);
    assert_eq!(duties[0].target.commitment, commitment(0xB4));
}

#[tokio::test]
async fn a_duty_pass_creates_the_local_bucket_before_the_first_checkpoint() {
    // The chain has no commitment to sync to yet, and in the second case the
    // sync balance is too low for a sync.
    for sync_balance in [1000, 10] {
        let mock = MockReplicaSyncChainClient::new().with_agreements(vec![ReplicaAgreementInfo {
            bucket_id: 42,
            sync_balance,
            sync_price: 100,
            min_sync_interval: 0,
            last_sync: None,
            max_bytes: 1024 * 1024,
            expires_at: 10_000,
        }]);
        let (storage, _dir) = temp_rocksdb();
        let coordinator = coordinator(
            ReplicaSyncCoordinatorConfig::default(),
            Arc::clone(&storage),
            mock,
        );

        let duties = coordinator.get_active_replica_duties().await.unwrap();

        assert!(duties.is_empty());
        // The bucket exists, so the first checkpoint of it starts a duty pass.
        assert!(storage.get_bucket(42).is_some(), "balance {sync_balance}");
    }
}

#[tokio::test]
async fn test_duty_sources_append_replicas_after_primaries_deduped() {
    // Primaries come first (most current); the bucket's other replicas follow
    // as the fallback for private buckets, and an endpoint that is both never
    // appears twice.
    let mock = mock_with_bucket_42_duty().with_replica_endpoints(Ok(vec![
        "http://primary:3333".to_string(),
        "http://replica:3334".to_string(),
    ]));

    let (storage, _dir) = temp_rocksdb();
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
    let mock = mock_with_bucket_42_duty().with_replica_endpoints(Err(ChainClientError::query(
        "bucket agreements",
        "chain down",
    )));

    let (storage, _dir) = temp_rocksdb();
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
    let (storage, _dir) = temp_rocksdb();
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
    let (storage, _dir) = temp_rocksdb();
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

// ─────────────────────────────────────────────────────────────────────────────
// Sync tasks: concurrency, timeout, Stop, Pause, expiry
// ─────────────────────────────────────────────────────────────────────────────

/// A mock chain with replica duties for buckets `1..=count`, all served by
/// `source`, which never answers in time.
fn mock_with_stalled_duties(count: u64, source: &str) -> MockReplicaSyncChainClient {
    let agreements = (1..=count)
        .map(|bucket_id| ReplicaAgreementInfo {
            bucket_id,
            sync_balance: 1000,
            sync_price: 100,
            min_sync_interval: 0,
            last_sync: None,
            max_bytes: 1024 * 1024,
            expires_at: 10_000,
        })
        .collect();
    let mut mock = MockReplicaSyncChainClient::new().with_agreements(agreements);
    for bucket_id in 1..=count {
        mock = mock
            .with_snapshot(
                bucket_id,
                BucketCommitments {
                    snapshot: Commitment {
                        mmr_root: H256::repeat_byte(bucket_id as u8),
                        start_seq: 0,
                        leaf_count: 1,
                    },
                    ..Default::default()
                },
            )
            .with_endpoints(bucket_id, vec![source.to_string()]);
    }
    mock
}

/// Poll `condition` for up to two seconds.
async fn eventually(mut condition: impl FnMut() -> bool) -> bool {
    for _ in 0..100 {
        if condition() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    false
}

fn syncing_count(syncing: &SyncingBuckets, buckets: u64) -> usize {
    (1..=buckets).filter(|id| syncing.is_syncing(*id)).count()
}

#[tokio::test]
async fn at_most_max_concurrent_syncs_run_at_once() {
    let stalled = spawn_slow(Duration::from_secs(30)).await;
    let mock = mock_with_stalled_duties(4, &stalled);
    let config = ReplicaSyncCoordinatorConfig {
        poll_interval: Duration::ZERO,
        max_concurrent_syncs: 2,
        ..Default::default()
    };
    let (storage, _dir) = temp_rocksdb();
    let syncing = SyncingBuckets::default();
    let (events_tx, events_rx) = tokio::sync::broadcast::channel(16);
    let handle = coordinator(config, storage, mock)
        .with_syncing_buckets(syncing.clone())
        .start(events_rx, None)
        .await
        .unwrap();

    events_tx
        .send(BlockEvent::Resubscribed { at_block: 1 })
        .unwrap();

    assert!(eventually(|| syncing_count(&syncing, 4) == 2).await);
    assert_eq!(handle.status().await.unwrap().active_syncs, 2);

    // Another pass starts nothing while both slots are taken.
    events_tx
        .send(BlockEvent::Resubscribed { at_block: 2 })
        .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(syncing_count(&syncing, 4), 2);

    handle.stop().await.unwrap();
}

#[tokio::test]
async fn stop_aborts_the_running_syncs() {
    let stalled = spawn_slow(Duration::from_secs(30)).await;
    let mock = mock_with_stalled_duties(2, &stalled);
    let config = ReplicaSyncCoordinatorConfig {
        poll_interval: Duration::ZERO,
        ..Default::default()
    };
    let (storage, _dir) = temp_rocksdb();
    let syncing = SyncingBuckets::default();
    let (events_tx, events_rx) = tokio::sync::broadcast::channel(16);
    let handle = coordinator(config, storage, mock)
        .with_syncing_buckets(syncing.clone())
        .start(events_rx, None)
        .await
        .unwrap();
    events_tx
        .send(BlockEvent::Resubscribed { at_block: 1 })
        .unwrap();
    assert!(eventually(|| syncing_count(&syncing, 2) == 2).await);

    handle.stop().await.unwrap();

    assert!(eventually(|| syncing_count(&syncing, 2) == 0).await);
}

#[tokio::test]
async fn pause_stops_the_running_syncs() {
    let stalled = spawn_slow(Duration::from_secs(30)).await;
    let mock = mock_with_stalled_duties(2, &stalled);
    let config = ReplicaSyncCoordinatorConfig {
        poll_interval: Duration::ZERO,
        ..Default::default()
    };
    let (storage, _dir) = temp_rocksdb();
    let syncing = SyncingBuckets::default();
    let (events_tx, events_rx) = tokio::sync::broadcast::channel(16);
    let handle = coordinator(config, storage, mock)
        .with_syncing_buckets(syncing.clone())
        .start(events_rx, None)
        .await
        .unwrap();
    events_tx
        .send(BlockEvent::Resubscribed { at_block: 1 })
        .unwrap();
    assert!(eventually(|| syncing_count(&syncing, 2) == 2).await);

    handle.pause().await.unwrap();

    assert!(eventually(|| syncing_count(&syncing, 2) == 0).await);
    handle.stop().await.unwrap();
}

#[tokio::test]
async fn pause_prevents_new_syncs_until_resume() {
    let stalled = spawn_slow(Duration::from_secs(30)).await;
    let mock = mock_with_stalled_duties(1, &stalled);
    let config = ReplicaSyncCoordinatorConfig {
        poll_interval: Duration::ZERO,
        ..Default::default()
    };
    let (storage, _dir) = temp_rocksdb();
    let syncing = SyncingBuckets::default();
    let (events_tx, events_rx) = tokio::sync::broadcast::channel(16);
    let handle = coordinator(config, storage, mock)
        .with_syncing_buckets(syncing.clone())
        .start(events_rx, None)
        .await
        .unwrap();

    handle.pause().await.unwrap();
    events_tx
        .send(BlockEvent::Resubscribed { at_block: 1 })
        .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(syncing_count(&syncing, 1), 0);

    // The event stayed queued, so resuming starts the sync.
    handle.resume().await.unwrap();
    assert!(eventually(|| syncing_count(&syncing, 1) == 1).await);

    handle.stop().await.unwrap();
}

#[tokio::test]
async fn a_sync_that_exceeds_the_timeout_ends_as_timed_out() {
    let stalled = spawn_slow(Duration::from_secs(30)).await;
    let duty = duty_from(
        &stalled,
        snapshot_target(H256::repeat_byte(0xAA), 1),
        1024 * 1024,
    );
    let config = ReplicaSyncCoordinatorConfig {
        sync_timeout: Duration::from_millis(200),
        ..Default::default()
    };
    let (storage, _dir) = temp_rocksdb();
    let coordinator = coordinator(config, storage, MockReplicaSyncChainClient::new());

    let result = coordinator.sync_and_confirm(&duty).await;

    assert!(
        matches!(result, SyncResult::TimedOut { bucket_id: 1 }),
        "{result:?}"
    );
}

#[tokio::test]
async fn a_duty_whose_agreement_expires_before_a_sync_can_finish_is_skipped() {
    // The default timeout of 300 s is 50 anchor blocks of 6 s; the mock chain
    // is at block 100, so a sync could end at block 150.
    for (expires_at, expected_duties) in [(150, 0), (151, 1)] {
        let mock = mock_with_bucket_42_duty();
        mock.agreements.lock().unwrap()[0].expires_at = expires_at;
        let (storage, _dir) = temp_rocksdb();
        let coordinator = coordinator(ReplicaSyncCoordinatorConfig::default(), storage, mock);

        let duties = coordinator.get_active_replica_duties().await.unwrap();

        assert_eq!(duties.len(), expected_duties, "expires_at {expires_at}");
    }
}

/// Start a coordinator over `mock` and force a sync of bucket 1. Returns the
/// syncing set, the handle, and the storage directory, which must outlive the
/// coordinator.
async fn force_sync_bucket_1(
    mock: MockReplicaSyncChainClient,
) -> (
    SyncingBuckets,
    provider_replica::ReplicaSyncCoordinatorHandle,
    tempfile::TempDir,
) {
    let config = ReplicaSyncCoordinatorConfig {
        poll_interval: Duration::ZERO,
        ..Default::default()
    };
    let (storage, _dir) = temp_rocksdb();
    let syncing = SyncingBuckets::default();
    let handle = coordinator(config, storage, mock)
        .with_syncing_buckets(syncing.clone())
        .start(tokio::sync::broadcast::channel(16).1, None)
        .await
        .unwrap();
    handle.force_sync(1).await.unwrap();
    (syncing, handle, _dir)
}

#[tokio::test]
async fn force_sync_ignores_the_sync_interval() {
    let stalled = spawn_slow(Duration::from_secs(30)).await;
    let mock = mock_with_stalled_duties(1, &stalled);
    // Synced one block ago, with an interval of 1000 blocks.
    {
        let mut agreements = mock.agreements.lock().unwrap();
        agreements[0].last_sync = Some((H256::repeat_byte(0xEE), 99));
        agreements[0].min_sync_interval = 1000;
    }

    let (syncing, handle, _dir) = force_sync_bucket_1(mock).await;

    assert!(eventually(|| syncing.is_syncing(1)).await);
    handle.stop().await.unwrap();
}

#[tokio::test]
async fn force_sync_does_not_start_for_an_expiring_agreement() {
    let stalled = spawn_slow(Duration::from_secs(30)).await;
    let mock = mock_with_stalled_duties(1, &stalled);
    // The chain is at block 100 and a sync may take 50 blocks.
    mock.agreements.lock().unwrap()[0].expires_at = 120;

    let (syncing, handle, _dir) = force_sync_bucket_1(mock).await;

    // The status reply comes after the force-sync command was handled.
    assert_eq!(handle.status().await.unwrap().active_syncs, 0);
    assert!(!syncing.is_syncing(1));
    handle.stop().await.unwrap();
}
