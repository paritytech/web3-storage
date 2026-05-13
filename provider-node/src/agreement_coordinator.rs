//! Agreement Coordinator - Auto-accept pending agreement requests.
//!
//! This module provides a background service that polls for pending
//! `AgreementRequests` on-chain and automatically accepts them on behalf
//! of the provider.

use crate::{Error, ProviderState};
use sp_core::crypto::Ss58Codec;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use subxt::{dynamic::Value, OnlineClient, PolkadotConfig};
use subxt_signer::sr25519::Keypair;
use tokio::sync::mpsc;

/// Configuration for the agreement coordinator.
#[derive(Clone, Debug)]
pub struct AgreementCoordinatorConfig {
    /// WebSocket URL for the parachain.
    pub chain_ws_url: String,
    /// How often to poll for pending agreement requests.
    pub poll_interval: Duration,
    /// Whether to automatically accept agreement requests.
    pub auto_accept: bool,
    /// Seed phrase or derivation path for signing (e.g., "//Alice").
    /// Used to create the subxt signer directly (avoids key conversion issues).
    pub seed: Option<String>,
}

impl Default for AgreementCoordinatorConfig {
    fn default() -> Self {
        Self {
            chain_ws_url: "ws://127.0.0.1:2222".to_string(),
            poll_interval: Duration::from_secs(6),
            auto_accept: true,
            seed: None,
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
    api: Option<OnlineClient<PolkadotConfig>>,
    signer: Option<Keypair>,
}

impl AgreementCoordinator {
    /// Create a new agreement coordinator.
    pub fn new(config: AgreementCoordinatorConfig, state: Arc<ProviderState>) -> Self {
        Self {
            config,
            state,
            api: None,
            signer: None,
        }
    }

    /// Connect to the blockchain.
    pub async fn connect(&mut self) -> Result<(), Error> {
        let api = OnlineClient::<PolkadotConfig>::from_url(&self.config.chain_ws_url)
            .await
            .map_err(|e| Error::Internal(format!("Failed to connect to chain: {e}")))?;

        self.api = Some(api);

        // Create signer from seed URI (e.g. "//Alice") using subxt_signer directly.
        // This avoids key conversion issues between sp_core and subxt_signer.
        if let Some(ref seed) = self.config.seed {
            let uri: subxt_signer::SecretUri = seed
                .parse()
                .map_err(|e| Error::Internal(format!("Invalid seed URI: {e}")))?;
            let signer = Keypair::from_uri(&uri)
                .map_err(|e| Error::Internal(format!("Failed to create signer: {e}")))?;
            tracing::info!(
                "Agreement coordinator signer: {}",
                sp_core::crypto::AccountId32::from(signer.public_key().0).to_ss58check()
            );
            self.signer = Some(signer);
        }

        tracing::info!(
            "Agreement coordinator connected to {}",
            self.config.chain_ws_url
        );
        Ok(())
    }

    /// Start the agreement coordinator background service.
    pub async fn start(self) -> Result<AgreementCoordinatorHandle, Error> {
        if self.api.is_none() {
            return Err(Error::Internal("Not connected to chain".to_string()));
        }
        if self.signer.is_none() {
            return Err(Error::Internal(
                "No signer available for agreement coordinator".to_string(),
            ));
        }

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
        let api = self
            .api
            .as_ref()
            .ok_or_else(|| Error::Internal("Not connected".to_string()))?;
        let signer = self
            .signer
            .as_ref()
            .ok_or_else(|| Error::Internal("No signer".to_string()))?;

        let provider_id = &self.state.provider_id;

        // Convert our SS58 provider ID to raw AccountId32 bytes for key comparison
        let our_account: sp_core::crypto::AccountId32 =
            sp_core::crypto::Ss58Codec::from_ss58check(provider_id)
                .map_err(|e| Error::Internal(format!("Invalid provider SS58 address: {e:?}")))?;
        let our_bytes: [u8; 32] = our_account.into();

        // Iterate ALL AgreementRequests entries on chain.
        // Storage layout: DoubleMap<Blake2_128Concat(BucketId), Blake2_128Concat(AccountId), Request>
        // Key bytes: [16 pallet_hash][16 storage_hash][16 blake2_hash + 8 bucket_id][16 blake2_hash + 32 account]
        // Total = 32 (prefix) + 24 (key1) + 48 (key2) = 104 bytes
        let storage_query = subxt::dynamic::storage("StorageProvider", "AgreementRequests", ());
        let storage = api
            .storage()
            .at_latest()
            .await
            .map_err(|e| Error::Internal(format!("Failed to get storage: {e}")))?;

        let mut entries = storage
            .iter(storage_query)
            .await
            .map_err(|e| Error::Internal(format!("Failed to iterate agreement requests: {e}")))?;

        let mut bucket_ids_to_accept: Vec<u64> = Vec::new();
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

            // Check if this request is for our provider
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

            bucket_ids_to_accept.push(bucket_id);
        }

        if entry_count > 0 {
            tracing::info!(
                "Scanned {} agreement request entries, {} for us",
                entry_count,
                bucket_ids_to_accept.len()
            );
        }

        // Accept each pending request
        for bucket_id in bucket_ids_to_accept {
            tracing::info!("Auto-accepting agreement for bucket {}", bucket_id);

            let tx = subxt::dynamic::tx(
                "StorageProvider",
                "accept_agreement",
                vec![Value::u128(bucket_id as u128)],
            );

            match api
                .tx()
                .sign_and_submit_then_watch_default(&tx, signer)
                .await
            {
                Ok(progress) => match progress.wait_for_finalized_success().await {
                    Ok(_events) => {
                        tracing::info!(
                            "Auto-accepted agreement for bucket {} (finalized)",
                            bucket_id
                        );
                    }
                    Err(e) => {
                        tracing::warn!(
                            "accept_agreement tx failed for bucket {}: {}",
                            bucket_id,
                            e
                        );
                    }
                },
                Err(e) => {
                    tracing::warn!(
                        "Failed to submit accept_agreement for bucket {}: {}",
                        bucket_id,
                        e
                    );
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = AgreementCoordinatorConfig::default();
        assert_eq!(config.chain_ws_url, "ws://127.0.0.1:2222");
        assert_eq!(config.poll_interval, Duration::from_secs(6));
        assert!(config.auto_accept);
    }

    #[test]
    fn test_coordinator_creation() {
        let storage = Arc::new(crate::Storage::new());
        let state = Arc::new(crate::ProviderState::new(
            storage,
            "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY".to_string(),
        ));
        let config = AgreementCoordinatorConfig::default();
        let coordinator = AgreementCoordinator::new(config, state);
        assert!(coordinator.api.is_none());
        assert!(coordinator.signer.is_none());
    }
}
