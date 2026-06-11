//! Agreement Coordinator - Auto-accept pending agreement requests.
//!
//! This module provides a background service that polls for pending
//! `AgreementRequests` on-chain and automatically accepts them on behalf
//! of the provider.

use crate::{Error, ProviderState};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use storage_primitives::BucketId;
use tokio::sync::mpsc;

/// Trait abstracting chain interactions for the agreement coordinator.
///
/// Stored as `Box<dyn AgreementChainClient>`, enabling mock-based testing
/// without a live chain.
#[async_trait::async_trait]
pub trait AgreementChainClient: Send + Sync {
    /// Fetch bucket IDs with pending agreement requests for this provider.
    async fn fetch_pending_requests(
        &self,
        provider_account: &[u8; 32],
    ) -> Result<Vec<BucketId>, Error>;

    /// Accept a pending agreement request for the given bucket.
    async fn accept_agreement(&self, bucket_id: BucketId) -> Result<(), Error>;
}

#[async_trait::async_trait]
impl<T: AgreementChainClient> AgreementChainClient for Arc<T> {
    async fn fetch_pending_requests(
        &self,
        provider_account: &[u8; 32],
    ) -> Result<Vec<BucketId>, Error> {
        self.as_ref().fetch_pending_requests(provider_account).await
    }

    async fn accept_agreement(&self, bucket_id: BucketId) -> Result<(), Error> {
        self.as_ref().accept_agreement(bucket_id).await
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
                // Prefer control commands over the poll tick: the interval's
                // first tick fires immediately, so an unbiased select could
                // service a poll before a Stop queued right after start().
                biased;

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
    pub async fn poll_and_accept(&self) -> Result<(), Error> {
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
