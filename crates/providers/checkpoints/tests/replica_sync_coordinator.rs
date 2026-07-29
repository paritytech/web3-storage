// SPDX-License-Identifier: Apache-2.0

//! Coordinator tests behind a [`ReplicaStore`] fake, covering the post-sync
//! verification and submission paths that the provider node's integration
//! tests cannot reach (their mock primaries never complete a sync).

use provider_checkpoints::{
    BucketSnapshot, Error, ReplicaAgreementInfo, ReplicaStore, ReplicaSyncChainClient,
    ReplicaSyncCoordinator, ReplicaSyncCoordinatorConfig, SyncDuty, SyncResult,
};
use sp_core::H256;
use std::sync::{Arc, Mutex};
use storage_primitives::BucketId;

/// [`ReplicaStore`] fake with a controllable local root and sync outcome.
struct TmpStore {
    /// Root reported by `local_mmr_root`.
    local_root: Mutex<Option<H256>>,
    /// Root `sync_from_primary` reports as synced.
    synced_root: H256,
    /// When set, a successful sync also updates `local_root` (models the
    /// read-after-write contract between the two methods).
    write_on_sync: bool,
}

impl TmpStore {
    fn new(local_root: Option<H256>, synced_root: H256, write_on_sync: bool) -> Arc<Self> {
        Arc::new(Self {
            local_root: Mutex::new(local_root),
            synced_root,
            write_on_sync,
        })
    }
}

#[async_trait::async_trait]
impl ReplicaStore for TmpStore {
    fn provider_id(&self) -> String {
        "fake-provider".to_string()
    }

    async fn local_bucket_ids(&self) -> Vec<BucketId> {
        vec![1]
    }

    async fn local_mmr_root(&self, _bucket_id: BucketId) -> Option<H256> {
        *self.local_root.lock().unwrap()
    }

    async fn sync_from_primary(
        &self,
        _bucket_id: BucketId,
        _primary_url: &str,
    ) -> Result<H256, Error> {
        if self.write_on_sync {
            *self.local_root.lock().unwrap() = Some(self.synced_root);
        }
        Ok(self.synced_root)
    }
}

/// Minimal chain client: one replica agreement for bucket 1, with a
/// configurable snapshot root and submission outcome.
struct StubChainClient {
    snapshot_root: H256,
    submit_ok: bool,
}

#[async_trait::async_trait]
impl ReplicaSyncChainClient for StubChainClient {
    async fn get_current_block(&self) -> Result<u64, Error> {
        Ok(100)
    }

    async fn fetch_replica_agreements(
        &self,
        _provider_account: &str,
        _local_buckets: Vec<BucketId>,
    ) -> Result<Vec<ReplicaAgreementInfo>, Error> {
        Ok(vec![ReplicaAgreementInfo {
            bucket_id: 1,
            sync_balance: 1000,
            sync_price: 100,
            min_sync_interval: 0,
            last_sync: None,
        }])
    }

    async fn fetch_bucket_snapshot(&self, _bucket_id: BucketId) -> Result<BucketSnapshot, Error> {
        Ok(BucketSnapshot {
            mmr_root: self.snapshot_root,
            leaf_count: 1,
        })
    }

    async fn fetch_primary_endpoints(&self, _bucket_id: BucketId) -> Result<Vec<String>, Error> {
        Ok(vec!["http://primary:3333".to_string()])
    }

    async fn submit_sync_confirmation(
        &self,
        _bucket_id: BucketId,
        _target_mmr_root: H256,
    ) -> Result<(u8, u128), Error> {
        if self.submit_ok {
            Ok((0, 1000))
        } else {
            Err(Error::Chain("submission rejected".to_string()))
        }
    }
}

fn duty(target: H256) -> SyncDuty {
    SyncDuty {
        bucket_id: 1,
        target_mmr_root: target,
        target_leaf_count: 1,
        primary_endpoints: vec!["http://primary:3333".to_string()],
        sync_balance: 1000,
        sync_price: 100,
        min_sync_interval: 0,
        last_sync: None,
    }
}

fn coordinator(
    store: Arc<dyn ReplicaStore>,
    auto_confirm: bool,
    submit_ok: bool,
    snapshot_root: H256,
) -> ReplicaSyncCoordinator {
    let config = ReplicaSyncCoordinatorConfig {
        auto_confirm,
        ..Default::default()
    };
    let chain_client = StubChainClient {
        snapshot_root,
        submit_ok,
    };
    ReplicaSyncCoordinator::new(config, store, Box::new(chain_client))
}

/// Unwrap a [`SyncResult::VerificationFailed`], panicking on anything else.
fn verification_reason(result: SyncResult) -> String {
    match result {
        SyncResult::VerificationFailed { reason, .. } => reason,
        other => panic!("expected VerificationFailed, got {other:?}"),
    }
}

#[tokio::test]
async fn verification_fails_when_bucket_missing_after_sync() {
    let target = H256::repeat_byte(0xAA);
    // Sync "succeeds" but the store never exposes the bucket afterwards.
    let store = TmpStore::new(None, target, false);
    let coordinator = coordinator(store, true, true, target);

    let result = coordinator.sync_and_confirm(&duty(target)).await;
    assert_eq!(verification_reason(result), "Bucket not found after sync");
}

#[tokio::test]
async fn verification_fails_on_root_mismatch_after_sync() {
    let target = H256::repeat_byte(0xAA);
    let stale = H256::repeat_byte(0xBB);
    // Sync reports the target root but the store still shows a stale root.
    let store = TmpStore::new(Some(stale), target, false);
    let coordinator = coordinator(store, true, true, target);

    let result = coordinator.sync_and_confirm(&duty(target)).await;
    let reason = verification_reason(result);
    assert!(
        reason.starts_with("Root mismatch"),
        "unexpected reason: {reason}"
    );
}

#[tokio::test]
async fn sync_success_without_auto_confirm_skips_submission() {
    let target = H256::repeat_byte(0xAA);
    // Store honors read-after-write: verification sees what sync wrote.
    // submit_ok = false proves no submission happens on this path.
    let store = TmpStore::new(None, target, true);
    let coordinator = coordinator(store, false, false, target);

    let result = coordinator.sync_and_confirm(&duty(target)).await;
    assert!(matches!(result, SyncResult::Success { payment: 0, .. }));
}

#[tokio::test]
async fn sync_success_with_auto_confirm_reports_payment() {
    let target = H256::repeat_byte(0xAA);
    let store = TmpStore::new(None, target, true);
    let coordinator = coordinator(store, true, true, target);

    let result = coordinator.sync_and_confirm(&duty(target)).await;
    assert!(matches!(result, SyncResult::Success { payment: 1000, .. }));
}

#[tokio::test]
async fn submission_failure_is_reported() {
    let target = H256::repeat_byte(0xAA);
    let store = TmpStore::new(None, target, true);
    let coordinator = coordinator(store, true, false, target);

    let result = coordinator.sync_and_confirm(&duty(target)).await;
    assert!(matches!(result, SyncResult::SubmissionFailed { .. }));
}

#[tokio::test]
async fn duties_returned_for_unsynced_bucket() {
    let target = H256::repeat_byte(0xAA);
    // Local storage has nothing yet; the chain snapshot points at `target`.
    let store = TmpStore::new(None, target, true);
    let coordinator = coordinator(store, true, true, target);

    let duties = coordinator.get_active_replica_duties().await.unwrap();
    assert_eq!(duties.len(), 1);
    assert_eq!(duties[0].bucket_id, 1);
    assert_eq!(duties[0].target_mmr_root, target);
    assert_eq!(duties[0].primary_endpoints, vec!["http://primary:3333"]);
}
