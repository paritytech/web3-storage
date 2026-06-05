//! Agreement Coordinator - Auto-accept pending agreement requests.
//!
//! This module provides a background service that polls for pending
//! `AgreementRequests` on-chain and automatically accepts them on behalf
//! of the provider.

use crate::{Error, ProviderState};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use storage_primitives::BucketId;
use tokio::sync::mpsc;

/// Trait abstracting chain interactions for the agreement coordinator.
///
/// Follows the same pattern as [`crate::auth::MembershipResolver`]: a trait
/// with `Pin<Box<dyn Future>>` return types, stored as `Box<dyn AgreementChainClient>`,
/// enabling mock-based testing without a live chain.
pub trait AgreementChainClient: Send + Sync {
    /// Fetch bucket IDs with pending agreement requests for this provider.
    fn fetch_pending_requests(
        &self,
        provider_account: &[u8; 32],
    ) -> Pin<Box<dyn Future<Output = Result<Vec<BucketId>, Error>> + Send + '_>>;

    /// Accept a pending agreement request for the given bucket.
    fn accept_agreement(
        &self,
        bucket_id: BucketId,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + '_>>;
}

/// Production implementation that talks to the chain via subxt.
pub struct SubxtAgreementChainClient {
    api: subxt::OnlineClient<subxt::PolkadotConfig>,
    signer: subxt_signer::sr25519::Keypair,
}

impl SubxtAgreementChainClient {
    /// Connect to the chain and create a signer from the seed URI.
    pub async fn connect(chain_ws_url: &str, seed: &str) -> Result<Self, Error> {
        let api = subxt::OnlineClient::<subxt::PolkadotConfig>::from_url(chain_ws_url)
            .await
            .map_err(|e| Error::Internal(format!("Failed to connect to chain: {e}")))?;

        let uri: subxt_signer::SecretUri = seed
            .parse()
            .map_err(|e| Error::Internal(format!("Invalid seed URI: {e}")))?;
        let signer = subxt_signer::sr25519::Keypair::from_uri(&uri)
            .map_err(|e| Error::Internal(format!("Failed to create signer: {e}")))?;

        tracing::info!(
            "Agreement coordinator signer: {}",
            sp_core::crypto::AccountId32::from(signer.public_key().0).to_string()
        );
        tracing::info!("Agreement coordinator connected to {}", chain_ws_url);

        Ok(Self { api, signer })
    }
}

impl AgreementChainClient for SubxtAgreementChainClient {
    fn fetch_pending_requests(
        &self,
        provider_account: &[u8; 32],
    ) -> Pin<Box<dyn Future<Output = Result<Vec<BucketId>, Error>> + Send + '_>> {
        let our_bytes = *provider_account;
        Box::pin(async move {
            // Iterate ALL AgreementRequests entries on chain.
            // Storage layout: DoubleMap<Blake2_128Concat(BucketId), Blake2_128Concat(AccountId), Request>
            // Key bytes: [16 pallet_hash][16 storage_hash][16 blake2_hash + 8 bucket_id][16 blake2_hash + 32 account]
            // Total = 32 (prefix) + 24 (key1) + 48 (key2) = 104 bytes
            let storage_query = subxt::dynamic::storage("StorageProvider", "AgreementRequests", ());
            let storage = self
                .api
                .storage()
                .at_latest()
                .await
                .map_err(|e| Error::Internal(format!("Failed to get storage: {e}")))?;

            let mut entries = storage.iter(storage_query).await.map_err(|e| {
                Error::Internal(format!("Failed to iterate agreement requests: {e}"))
            })?;

            let mut bucket_ids = Vec::new();
            let mut entry_count = 0u32;

            while let Some(result) = entries.next().await {
                let entry = match result {
                    Ok(e) => e,
                    Err(e) => {
                        tracing::warn!("Error reading agreement request entry: {}", e);
                        continue;
                    }
                };

                entry_count += 1;
                let key_bytes = &entry.key_bytes;
                let key_len = key_bytes.len();

                // Expected key length: 32 (prefix) + 24 (key1) + 48 (key2) = 104
                if key_len < 104 {
                    tracing::warn!("Unexpected key length {} (expected 104), skipping", key_len);
                    continue;
                }

                // Account bytes at offset 72 (32 prefix + 16 blake2 + 8 bucket + 16 blake2)
                let account_bytes = &key_bytes[72..104];

                if account_bytes != our_bytes.as_slice() {
                    continue;
                }

                // Bucket ID at offset 48 (32 prefix + 16 blake2 hash)
                let bucket_id = u64::from_le_bytes(
                    key_bytes[48..56]
                        .try_into()
                        .expect("slice is exactly 8 bytes"),
                );

                tracing::info!(
                    "Found pending agreement request for us: bucket {}",
                    bucket_id
                );
                bucket_ids.push(bucket_id);
            }

            if entry_count > 0 {
                tracing::info!(
                    "Scanned {} agreement request entries, {} for us",
                    entry_count,
                    bucket_ids.len()
                );
            }

            Ok(bucket_ids)
        })
    }

    fn accept_agreement(
        &self,
        bucket_id: BucketId,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + '_>> {
        Box::pin(async move {
            let tx = subxt::dynamic::tx(
                "StorageProvider",
                "accept_agreement",
                vec![subxt::dynamic::Value::u128(bucket_id as u128)],
            );

            let progress = self
                .api
                .tx()
                .sign_and_submit_then_watch_default(&tx, &self.signer)
                .await
                .map_err(|e| {
                    Error::Internal(format!(
                        "Failed to submit accept_agreement for bucket {bucket_id}: {e}"
                    ))
                })?;

            progress.wait_for_finalized_success().await.map_err(|e| {
                Error::Internal(format!(
                    "accept_agreement tx failed for bucket {bucket_id}: {e}"
                ))
            })?;

            tracing::info!(
                "Auto-accepted agreement for bucket {} (finalized)",
                bucket_id
            );
            Ok(())
        })
    }
}

/// Configuration for the agreement coordinator.
#[derive(Clone, Debug)]
pub struct AgreementCoordinatorConfig {
    /// How often to poll for pending agreement requests.
    pub poll_interval: Duration,
    /// Whether to automatically accept agreement requests.
    pub auto_accept: bool,
}

impl Default for AgreementCoordinatorConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(6),
            auto_accept: true,
        }
    }
}

/// Commands for controlling the coordinator.
#[derive(Debug)]
pub enum AgreementCommand {
    /// Stop the coordinator.
    Stop,
}

/// Handle for controlling the agreement coordinator.
pub struct AgreementCoordinatorHandle {
    command_tx: mpsc::Sender<AgreementCommand>,
    running: Arc<AtomicBool>,
}

impl AgreementCoordinatorHandle {
    /// Check if the coordinator is running.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Stop the coordinator.
    pub async fn stop(&self) -> Result<(), Error> {
        self.command_tx
            .send(AgreementCommand::Stop)
            .await
            .map_err(|_| Error::Internal("Agreement coordinator channel closed".to_string()))
    }
}

/// Agreement coordinator service.
pub struct AgreementCoordinator {
    config: AgreementCoordinatorConfig,
    state: Arc<ProviderState>,
    chain_client: Box<dyn AgreementChainClient>,
}

impl AgreementCoordinator {
    /// Create a new agreement coordinator.
    pub fn new(
        config: AgreementCoordinatorConfig,
        state: Arc<ProviderState>,
        chain_client: Box<dyn AgreementChainClient>,
    ) -> Self {
        Self {
            config,
            state,
            chain_client,
        }
    }

    /// Start the agreement coordinator background service.
    pub async fn start(self) -> Result<AgreementCoordinatorHandle, Error> {
        let (command_tx, command_rx) = mpsc::channel::<AgreementCommand>(32);
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();

        tokio::spawn(async move {
            self.run_loop(command_rx, running_clone).await;
        });

        Ok(AgreementCoordinatorHandle {
            command_tx,
            running,
        })
    }

    /// Main coordinator loop.
    async fn run_loop(
        self,
        mut command_rx: mpsc::Receiver<AgreementCommand>,
        running: Arc<AtomicBool>,
    ) {
        let mut interval = tokio::time::interval(self.config.poll_interval);

        tracing::info!("Agreement coordinator started");

        loop {
            tokio::select! {
                cmd = command_rx.recv() => {
                    match cmd {
                        Some(AgreementCommand::Stop) | None => {
                            tracing::info!("Agreement coordinator stopping");
                            running.store(false, Ordering::SeqCst);
                            break;
                        }
                    }
                }
                _ = interval.tick() => {
                    if !self.config.auto_accept {
                        continue;
                    }

                    if let Err(e) = self.poll_and_accept().await {
                        tracing::warn!("Agreement poll error: {}", e);
                    }
                }
            }
        }
    }

    /// Poll for pending agreement requests and accept them.
    async fn poll_and_accept(&self) -> Result<(), Error> {
        let provider_id = &self.state.provider_id;

        // Convert our SS58 provider ID to raw AccountId32 bytes for key comparison
        let our_account: sp_core::crypto::AccountId32 =
            sp_core::crypto::Ss58Codec::from_ss58check(provider_id)
                .map_err(|e| Error::Internal(format!("Invalid provider SS58 address: {e:?}")))?;
        let our_bytes: [u8; 32] = our_account.into();

        let bucket_ids = self.chain_client.fetch_pending_requests(&our_bytes).await?;

        for bucket_id in bucket_ids {
            tracing::info!("Auto-accepting agreement for bucket {}", bucket_id);
            if let Err(e) = self.chain_client.accept_agreement(bucket_id).await {
                tracing::warn!("Failed to accept agreement for bucket {}: {}", bucket_id, e);
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    impl AgreementChainClient for MockAgreementChainClient {
        fn fetch_pending_requests(
            &self,
            _provider_account: &[u8; 32],
        ) -> Pin<Box<dyn Future<Output = Result<Vec<BucketId>, Error>> + Send + '_>> {
            Box::pin(async { Ok(self.pending.lock().await.clone()) })
        }

        fn accept_agreement(
            &self,
            bucket_id: BucketId,
        ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + '_>> {
            Box::pin(async move {
                let err = self.accept_error.lock().await.take();
                if let Some(e) = err {
                    return Err(e);
                }
                self.accepted.lock().await.push(bucket_id);
                Ok(())
            })
        }
    }

    fn test_state() -> Arc<ProviderState> {
        let storage = Arc::new(crate::Storage::new());
        Arc::new(crate::ProviderState::new(
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
        let coordinator = AgreementCoordinator::new(config, state, Box::new(Arc::clone(&mock)));

        coordinator.poll_and_accept().await.unwrap();

        assert!(mock.accepted.lock().await.is_empty());
    }

    #[tokio::test]
    async fn test_two_pending_accepted() {
        let mock = Arc::new(MockAgreementChainClient::new(vec![1, 2]));
        let state = test_state();
        let config = AgreementCoordinatorConfig::default();
        let coordinator = AgreementCoordinator::new(config, state, Box::new(Arc::clone(&mock)));

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
        let coordinator = AgreementCoordinator::new(config, state, Box::new(Arc::clone(&mock)));

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
        let coordinator = AgreementCoordinator::new(config, state, Box::new(Arc::clone(&mock)));

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

    // Wrapper to forward trait calls through an Arc
    impl AgreementChainClient for Arc<MockAgreementChainClient> {
        fn fetch_pending_requests(
            &self,
            provider_account: &[u8; 32],
        ) -> Pin<Box<dyn Future<Output = Result<Vec<BucketId>, Error>> + Send + '_>> {
            (**self).fetch_pending_requests(provider_account)
        }

        fn accept_agreement(
            &self,
            bucket_id: BucketId,
        ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + '_>> {
            (**self).accept_agreement(bucket_id)
        }
    }
}
