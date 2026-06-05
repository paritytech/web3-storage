//! Integration tests for the checkpoint coordinator (moved from unit tests for CI coverage).

use sp_core::H256;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use storage_primitives::BucketId;
use storage_provider_node::checkpoint_coordinator::SignProposalRequest;
use storage_provider_node::{
    CheckpointChainClient, CheckpointCoordinator, CheckpointCoordinatorConfig, CheckpointDuty,
    CheckpointResult, Error, ProviderState, Storage,
};
use tokio::sync::Mutex;

struct MockCheckpointChainClient {
    block_number: Mutex<u64>,
    config: Mutex<Option<(u32, u32)>>,
    submitted: Mutex<Vec<(BucketId, u64)>>,
    submit_result: Mutex<Result<H256, Error>>,
}

impl MockCheckpointChainClient {
    fn new(block: u64) -> Self {
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

impl CheckpointChainClient for MockCheckpointChainClient {
    fn get_current_block(&self) -> Pin<Box<dyn Future<Output = Result<u64, Error>> + Send + '_>> {
        Box::pin(async { Ok(*self.block_number.lock().await) })
    }

    fn fetch_checkpoint_config(
        &self,
        _bucket_id: BucketId,
    ) -> Pin<Box<dyn Future<Output = Result<Option<(u32, u32)>, Error>> + Send + '_>> {
        Box::pin(async { Ok(*self.config.lock().await) })
    }

    fn submit_checkpoint(
        &self,
        duty: &CheckpointDuty,
        _signatures: Vec<(String, String)>,
    ) -> Pin<Box<dyn Future<Output = Result<H256, Error>> + Send + '_>> {
        let bucket_id = duty.bucket_id;
        let window = duty.window;
        Box::pin(async move {
            self.submitted.lock().await.push((bucket_id, window));
            let mut result = self.submit_result.lock().await;
            match &*result {
                Ok(h) => Ok(*h),
                Err(e) => {
                    let err = Error::Internal(e.to_string());
                    *result = Ok(H256::zero());
                    Err(err)
                }
            }
        })
    }
}

/// Newtype wrapper to satisfy orphan rules when impl'ing the trait for shared mock access.
struct SharedMock(Arc<MockCheckpointChainClient>);

impl CheckpointChainClient for SharedMock {
    fn get_current_block(&self) -> Pin<Box<dyn Future<Output = Result<u64, Error>> + Send + '_>> {
        self.0.get_current_block()
    }

    fn fetch_checkpoint_config(
        &self,
        bucket_id: BucketId,
    ) -> Pin<Box<dyn Future<Output = Result<Option<(u32, u32)>, Error>> + Send + '_>> {
        self.0.fetch_checkpoint_config(bucket_id)
    }

    fn submit_checkpoint(
        &self,
        duty: &CheckpointDuty,
        signatures: Vec<(String, String)>,
    ) -> Pin<Box<dyn Future<Output = Result<H256, Error>> + Send + '_>> {
        self.0.submit_checkpoint(duty, signatures)
    }
}

fn test_state_with_seed() -> Arc<ProviderState> {
    let storage = Arc::new(Storage::new());
    Arc::new(ProviderState::with_seed(storage, "//Alice").unwrap())
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
fn test_checkpoint_result_variants() {
    let success = CheckpointResult::Success {
        bucket_id: 1,
        window: 5,
        mmr_root: H256::zero(),
        signers: vec!["alice".to_string()],
    };
    assert!(matches!(success, CheckpointResult::Success { .. }));

    let insufficient = CheckpointResult::InsufficientSignatures {
        bucket_id: 1,
        window: 5,
        collected: 1,
        required: 3,
    };
    assert!(matches!(
        insufficient,
        CheckpointResult::InsufficientSignatures { .. }
    ));
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
    let state = test_state_with_seed();
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
    let coordinator =
        CheckpointCoordinator::new(config, state, Box::new(SharedMock(Arc::clone(&mock))));

    let duty = coordinator.get_checkpoint_duty(1).await.unwrap().unwrap();
    assert_eq!(duty.bucket_id, 1);
    assert_eq!(duty.window, 5); // 500 / 100

    let result = coordinator.coordinate_checkpoint(&duty).await;
    assert!(matches!(result, CheckpointResult::Success { .. }));

    let submitted = mock.submitted.lock().await;
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
    let coordinator =
        CheckpointCoordinator::new(config, state, Box::new(SharedMock(Arc::clone(&mock))));

    let duty = coordinator.get_checkpoint_duty(1).await.unwrap().unwrap();
    let result = coordinator.coordinate_checkpoint(&duty).await;
    assert!(matches!(result, CheckpointResult::SubmissionFailed { .. }));
}

#[tokio::test]
async fn test_pause_resume() {
    let mock = MockCheckpointChainClient::new(500);
    let state = test_state_with_seed();
    let config = CheckpointCoordinatorConfig {
        poll_interval: Duration::from_millis(50),
        ..Default::default()
    };
    let coordinator = CheckpointCoordinator::new(config, state, Box::new(mock));

    let handle = coordinator.start(None).await.unwrap();
    assert!(handle.is_running());

    handle.pause().await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    handle.resume().await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    handle.stop().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!handle.is_running());
}

#[tokio::test]
async fn test_force_checkpoint() {
    let mock = Arc::new(MockCheckpointChainClient::new(500));
    let state = test_state_with_bucket(1);
    let config = CheckpointCoordinatorConfig {
        poll_interval: Duration::from_secs(60),
        ..Default::default()
    };
    let coordinator =
        CheckpointCoordinator::new(config, state, Box::new(SharedMock(Arc::clone(&mock))));

    let handle = coordinator.start(None).await.unwrap();

    handle.force_checkpoint(1).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    let submitted = mock.submitted.lock().await;
    assert_eq!(submitted.len(), 1);

    handle.stop().await.unwrap();
}
