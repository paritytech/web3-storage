// SPDX-License-Identifier: GPL-3.0-only

//! Integration tests for the checkpoint coordinator.

use super::{test_state_with_seed, wait_for};
use sp_core::H256;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use storage_primitives::BucketId;
use storage_provider_node::checkpoint_coordinator::SignProposalRequest;
use storage_provider_node::{
    CheckpointChainClient, CheckpointCoordinator, CheckpointCoordinatorConfig, CheckpointDuty,
    CheckpointResult, Error, ProviderState, Storage,
};

pub(crate) struct MockCheckpointChainClient {
    block_number: Mutex<u64>,
    config: Mutex<Option<(u32, u32)>>,
    submitted: Mutex<Vec<(BucketId, u64)>>,
    submit_result: Mutex<Result<H256, Error>>,
}

impl MockCheckpointChainClient {
    pub(crate) fn new(block: u64) -> Self {
        Self {
            block_number: Mutex::new(block),
            config: Mutex::new(Some((100, 20))),
            submitted: Mutex::new(Vec::new()),
            submit_result: Mutex::new(Ok(H256::zero())),
        }
    }

    fn with_submit_error(self, err: Error) -> Self {
        Self {
            submit_result: Mutex::new(Err(err)),
            ..self
        }
    }
}

#[async_trait::async_trait]
impl CheckpointChainClient for MockCheckpointChainClient {
    async fn get_current_block(&self) -> Result<u64, Error> {
        Ok(*self.block_number.lock().unwrap())
    }

    async fn fetch_checkpoint_config(
        &self,
        _bucket_id: BucketId,
    ) -> Result<Option<(u32, u32)>, Error> {
        Ok(*self.config.lock().unwrap())
    }

    async fn submit_checkpoint(
        &self,
        duty: &CheckpointDuty,
        _signatures: Vec<(String, String)>,
    ) -> Result<H256, Error> {
        let bucket_id = duty.bucket_id;
        let window = duty.window;
        self.submitted.lock().unwrap().push((bucket_id, window));
        let mut result = self.submit_result.lock().unwrap();
        match &*result {
            Ok(h) => Ok(*h),
            Err(e) => {
                let err = Error::Internal(e.to_string());
                *result = Ok(H256::zero());
                Err(err)
            }
        }
    }
}

fn test_state_with_bucket(bucket_id: BucketId) -> Arc<ProviderState> {
    let storage = Arc::new(Storage::new());
    storage.init_bucket(bucket_id, 1024 * 1024);
    let data = b"test data".to_vec();
    let hash = sp_core::hashing::blake2_256(&data);
    let data_root = H256::from(hash);
    let _ = storage.store_node(bucket_id, data_root, data, None);
    storage.commit(bucket_id, vec![data_root]).unwrap();
    Arc::new(ProviderState::with_seed(storage, "//Alice").unwrap())
}

#[test]
fn test_config_default() {
    let config = CheckpointCoordinatorConfig::default();
    assert_eq!(config.poll_interval, Duration::from_secs(6));
    assert!(config.auto_submit);
}

#[test]
fn test_sign_proposal_request_serialization() {
    let request = SignProposalRequest {
        bucket_id: 1,
        mmr_root: "0x0000000000000000000000000000000000000000000000000000000000000000".to_string(),
        start_seq: 0,
        leaf_count: 10,
        window: 5,
    };

    let json = serde_json::to_string(&request).unwrap();
    assert!(json.contains("bucket_id"));
    assert!(json.contains("mmr_root"));
}

#[tokio::test]
async fn test_no_bucket_data() {
    let mock = MockCheckpointChainClient::new(500);
    let state = test_state_with_seed("//Alice");
    let config = CheckpointCoordinatorConfig::default();
    let coordinator = CheckpointCoordinator::new(config, state, Box::new(mock));

    let duty = coordinator.get_checkpoint_duty(99).await.unwrap();
    assert!(duty.is_none());
}

#[tokio::test]
async fn test_duty_found_submit_ok() {
    let mock = Arc::new(MockCheckpointChainClient::new(500));
    let state = test_state_with_bucket(1);
    let config = CheckpointCoordinatorConfig::default();
    let coordinator = CheckpointCoordinator::new(config, state, Box::new(Arc::clone(&mock)));

    let duty = coordinator.get_checkpoint_duty(1).await.unwrap().unwrap();
    assert_eq!(duty.bucket_id, 1);
    assert_eq!(duty.window, 5); // 500 / 100

    let result = coordinator.coordinate_checkpoint(&duty).await;
    assert!(matches!(result, CheckpointResult::Success { .. }));

    let submitted = mock.submitted.lock().unwrap();
    assert_eq!(submitted.len(), 1);
    assert_eq!(submitted[0], (1, 5));
}

#[tokio::test]
async fn test_submit_fails() {
    let mock = Arc::new(
        MockCheckpointChainClient::new(500)
            .with_submit_error(Error::Internal("tx failed".to_string())),
    );
    let state = test_state_with_bucket(1);
    let config = CheckpointCoordinatorConfig::default();
    let coordinator = CheckpointCoordinator::new(config, state, Box::new(Arc::clone(&mock)));

    let duty = coordinator.get_checkpoint_duty(1).await.unwrap().unwrap();
    let result = coordinator.coordinate_checkpoint(&duty).await;
    assert!(matches!(result, CheckpointResult::SubmissionFailed { .. }));
}

#[tokio::test(start_paused = true)]
async fn test_pause_resume() {
    let mock = MockCheckpointChainClient::new(500);
    let state = test_state_with_seed("//Alice");
    let config = CheckpointCoordinatorConfig {
        poll_interval: Duration::from_millis(50),
        ..Default::default()
    };
    let coordinator = CheckpointCoordinator::new(config, state, Box::new(mock));

    let handle = coordinator
        .start(tokio::sync::broadcast::channel(16).1, None)
        .await
        .unwrap();
    assert!(handle.is_running());

    handle.pause().await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    handle.resume().await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    handle.stop().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!handle.is_running());
}

#[tokio::test(start_paused = true)]
async fn test_force_checkpoint() {
    let mock = Arc::new(MockCheckpointChainClient::new(500));
    let state = test_state_with_bucket(1);
    let config = CheckpointCoordinatorConfig {
        poll_interval: Duration::from_secs(60),
        ..Default::default()
    };
    let coordinator = CheckpointCoordinator::new(config, state, Box::new(Arc::clone(&mock)));

    let handle = coordinator
        .start(tokio::sync::broadcast::channel(16).1, None)
        .await
        .unwrap();

    handle.force_checkpoint(1).await.unwrap();
    let mock_ref = Arc::clone(&mock);
    assert!(
        wait_for(5, 10, || {
            let m = Arc::clone(&mock_ref);
            async move { !m.submitted.lock().unwrap().is_empty() }
        })
        .await,
        "timed out waiting for checkpoint submission"
    );

    {
        let submitted = mock.submitted.lock().unwrap();
        assert_eq!(submitted.len(), 1);
    }

    handle.stop().await.unwrap();
}
