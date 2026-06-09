//! Integration tests for the agreement coordinator (moved from unit tests for CI coverage).

use std::sync::Arc;
use std::time::Duration;
use storage_primitives::BucketId;
use storage_provider_node::{
    AgreementChainClient, AgreementCoordinator, AgreementCoordinatorConfig, Error, ProviderState,
    Storage,
};
use tokio::sync::Mutex;

struct MockAgreementChainClient {
    pending: Mutex<Vec<BucketId>>,
    accepted: Mutex<Vec<BucketId>>,
    accept_error: Mutex<Option<Error>>,
}

impl MockAgreementChainClient {
    fn new(pending: Vec<BucketId>) -> Self {
        Self {
            pending: Mutex::new(pending),
            accepted: Mutex::new(Vec::new()),
            accept_error: Mutex::new(None),
        }
    }

    fn with_accept_error(mut self, err: Error) -> Self {
        self.accept_error = Mutex::new(Some(err));
        self
    }
}

#[async_trait::async_trait]
impl AgreementChainClient for MockAgreementChainClient {
    async fn fetch_pending_requests(
        &self,
        _provider_account: &[u8; 32],
    ) -> Result<Vec<BucketId>, Error> {
        Ok(self.pending.lock().await.clone())
    }

    async fn accept_agreement(&self, bucket_id: BucketId) -> Result<(), Error> {
        let err = self.accept_error.lock().await.take();
        if let Some(e) = err {
            return Err(e);
        }
        self.accepted.lock().await.push(bucket_id);
        Ok(())
    }
}

/// Newtype wrapper to satisfy orphan rules when impl'ing the trait for shared mock access.
struct SharedMock(Arc<MockAgreementChainClient>);

#[async_trait::async_trait]
impl AgreementChainClient for SharedMock {
    async fn fetch_pending_requests(
        &self,
        provider_account: &[u8; 32],
    ) -> Result<Vec<BucketId>, Error> {
        self.0.fetch_pending_requests(provider_account).await
    }

    async fn accept_agreement(&self, bucket_id: BucketId) -> Result<(), Error> {
        self.0.accept_agreement(bucket_id).await
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
fn test_default_config() {
    let config = AgreementCoordinatorConfig::default();
    assert_eq!(config.poll_interval, Duration::from_secs(6));
    assert!(config.auto_accept);
}

#[tokio::test]
async fn test_no_pending_requests() {
    let mock = Arc::new(MockAgreementChainClient::new(vec![]));
    let state = test_state();
    let config = AgreementCoordinatorConfig::default();
    let coordinator =
        AgreementCoordinator::new(config, state, Box::new(SharedMock(Arc::clone(&mock))));

    coordinator.poll_and_accept().await.unwrap();

    assert!(mock.accepted.lock().await.is_empty());
}

#[tokio::test]
async fn test_two_pending_accepted() {
    let mock = Arc::new(MockAgreementChainClient::new(vec![1, 2]));
    let state = test_state();
    let config = AgreementCoordinatorConfig::default();
    let coordinator =
        AgreementCoordinator::new(config, state, Box::new(SharedMock(Arc::clone(&mock))));

    coordinator.poll_and_accept().await.unwrap();

    let accepted = mock.accepted.lock().await;
    assert_eq!(*accepted, vec![1, 2]);
}

#[tokio::test]
async fn test_accept_fails_continues() {
    // When accept fails on the first bucket, the second should still be attempted
    let mock = Arc::new(
        MockAgreementChainClient::new(vec![1, 2])
            .with_accept_error(Error::Internal("chain error".to_string())),
    );
    let state = test_state();
    let config = AgreementCoordinatorConfig::default();
    let coordinator =
        AgreementCoordinator::new(config, state, Box::new(SharedMock(Arc::clone(&mock))));

    // Should not return error — individual failures are logged, not propagated
    coordinator.poll_and_accept().await.unwrap();

    // Bucket 1 triggers the error (consumed), bucket 2 succeeds
    let accepted = mock.accepted.lock().await;
    assert_eq!(*accepted, vec![2]);
}

#[tokio::test]
async fn test_auto_accept_disabled() {
    let mock = Arc::new(MockAgreementChainClient::new(vec![1]));
    let state = test_state();
    let config = AgreementCoordinatorConfig {
        auto_accept: false,
        poll_interval: Duration::from_millis(50),
    };
    let coordinator =
        AgreementCoordinator::new(config, state, Box::new(SharedMock(Arc::clone(&mock))));

    let handle = coordinator.start().await.unwrap();

    // Give the loop a few ticks
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Nothing should have been accepted because auto_accept is false
    assert!(mock.accepted.lock().await.is_empty());

    handle.stop().await.unwrap();
}

#[tokio::test]
async fn test_stop_command() {
    let mock = MockAgreementChainClient::new(vec![]);
    let state = test_state();
    let config = AgreementCoordinatorConfig {
        poll_interval: Duration::from_secs(60),
        ..Default::default()
    };
    let coordinator = AgreementCoordinator::new(config, state, Box::new(mock));

    let handle = coordinator.start().await.unwrap();
    assert!(handle.is_running());

    handle.stop().await.unwrap();
    // Give the loop time to process the stop
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!handle.is_running());
}
