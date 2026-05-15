//! Agreement Coordinator — auto-accept pending agreement requests.
//!
//! Subscribes to finalized blocks and reacts to `AgreementRequested` events targeted at
//! this provider. On startup, performs a one-time storage snapshot at the first finalized
//! block to pick up requests that existed before the subscription began.

use crate::chain_stream::{field_account, field_u64, BlockTick, ChainStream};
use crate::{Error, ProviderState};
use sp_core::crypto::Ss58Codec;
use sp_core::H256;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use subxt::{dynamic::Value as DynamicValue, OnlineClient, PolkadotConfig};
use subxt_signer::sr25519::Keypair;
use tokio::sync::mpsc;

/// Configuration for the agreement coordinator.
#[derive(Clone, Debug)]
pub struct AgreementCoordinatorConfig {
    /// WebSocket URL for the parachain.
    pub chain_ws_url: String,
    /// Delay between reconnect attempts if the finalized-block subscription drops.
    /// Named `poll_interval` for backwards compatibility with the CLI flag.
    pub poll_interval: Duration,
    /// Whether to automatically accept agreement requests.
    pub auto_accept: bool,
    /// Seed phrase or derivation path for signing (e.g., "//Alice").
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

    /// Main loop: handle Stop, run the subscription, reconnect on failure.
    async fn run_loop(
        self,
        mut command_rx: mpsc::Receiver<AgreementCommand>,
        running: Arc<AtomicBool>,
    ) {
        tracing::info!("Agreement coordinator started");

        if !self.config.auto_accept {
            // Wait for Stop (or channel close). Either way, exit.
            let _ = command_rx.recv().await;
            running.store(false, Ordering::SeqCst);
            tracing::info!("Agreement coordinator stopping");
            return;
        }

        let api = match self.api.as_ref() {
            Some(a) => a.clone(),
            None => {
                tracing::error!("Agreement coordinator not connected to chain");
                running.store(false, Ordering::SeqCst);
                return;
            }
        };

        let our_bytes = match self.our_account_bytes() {
            Ok(b) => b,
            Err(e) => {
                tracing::error!("Agreement coordinator account error: {e}");
                running.store(false, Ordering::SeqCst);
                return;
            }
        };

        let mut stream = ChainStream::new(api.clone(), self.config.poll_interval);

        loop {
            tokio::select! {
                cmd = command_rx.recv() => {
                    match cmd {
                        Some(AgreementCommand::Stop) | None => break,
                    }
                }
                next = stream.next() => {
                    let Some(tick) = next else {
                        tracing::warn!("Agreement coordinator: chain stream ended");
                        break;
                    };

                    if let Err(e) = self.on_tick(&api, &tick, &our_bytes).await {
                        tracing::warn!("Agreement coordinator tick error: {e}");
                    }
                }
            }
        }

        running.store(false, Ordering::SeqCst);
        tracing::info!("Agreement coordinator stopping");
    }

    /// Process one `BlockTick` — initial snapshot on first tick after reconnect, event
    /// scan otherwise.
    async fn on_tick(
        &self,
        api: &OnlineClient<PolkadotConfig>,
        tick: &BlockTick,
        our_bytes: &[u8; 32],
    ) -> Result<(), Error> {
        if tick.is_first_after_reconnect {
            self.initial_snapshot(api, tick.hash(), our_bytes).await?;
        } else {
            self.process_events_in_block(api, &tick.block, our_bytes)
                .await;
        }
        Ok(())
    }

    fn our_account_bytes(&self) -> Result<[u8; 32], Error> {
        let our_account: sp_core::crypto::AccountId32 =
            sp_core::crypto::Ss58Codec::from_ss58check(&self.state.provider_id)
                .map_err(|e| Error::Internal(format!("Invalid provider SS58 address: {e:?}")))?;
        Ok(our_account.into())
    }

    /// Iterate `AgreementRequests` at a specific block and accept any pending requests
    /// targeted at our provider. Runs once on startup so we don't miss requests created
    /// before the subscription began.
    async fn initial_snapshot(
        &self,
        api: &OnlineClient<PolkadotConfig>,
        block_hash: H256,
        our_bytes: &[u8; 32],
    ) -> Result<(), Error> {
        // Storage layout: DoubleMap<Blake2_128Concat(BucketId), Blake2_128Concat(AccountId), Request>
        // Key bytes: [16 pallet_hash][16 storage_hash][16 blake2_hash + 8 bucket_id][16 blake2_hash + 32 account]
        // Total = 32 (prefix) + 24 (key1) + 48 (key2) = 104 bytes
        let storage_query = subxt::dynamic::storage("StorageProvider", "AgreementRequests", ());
        let storage = api.storage().at(block_hash);

        let mut entries = storage
            .iter(storage_query)
            .await
            .map_err(|e| Error::Internal(format!("Failed to iterate agreement requests: {e}")))?;

        let mut bucket_ids: Vec<u64> = Vec::new();
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
            if key_bytes.len() < 104 {
                tracing::warn!(
                    "Unexpected key length {} (expected 104), skipping",
                    key_bytes.len()
                );
                continue;
            }

            let account_bytes = &key_bytes[72..104];
            if account_bytes != our_bytes.as_slice() {
                continue;
            }

            let bucket_id = u64::from_le_bytes(
                key_bytes[48..56]
                    .try_into()
                    .expect("slice is exactly 8 bytes"),
            );

            tracing::info!(
                "Initial snapshot: pending agreement request for us, bucket {}",
                bucket_id
            );
            bucket_ids.push(bucket_id);
        }

        tracing::info!(
            "Initial snapshot at {:?}: scanned {} agreement request entries, {} for us",
            block_hash,
            entry_count,
            bucket_ids.len()
        );

        for bucket_id in bucket_ids {
            self.accept(api, bucket_id).await;
        }

        Ok(())
    }

    /// Walk a block's events, accepting any `AgreementRequested` whose `provider` field
    /// matches ours.
    async fn process_events_in_block(
        &self,
        api: &OnlineClient<PolkadotConfig>,
        block: &subxt::blocks::Block<PolkadotConfig, OnlineClient<PolkadotConfig>>,
        our_bytes: &[u8; 32],
    ) {
        let block_number = block.number();
        let events = match block.events().await {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!("Failed to fetch events at block {}: {}", block_number, e);
                return;
            }
        };

        for event_result in events.iter() {
            let event = match event_result {
                Ok(e) => e,
                Err(e) => {
                    tracing::trace!("Failed to decode event at block {}: {}", block_number, e);
                    continue;
                }
            };

            if event.pallet_name() != "StorageProvider"
                || event.variant_name() != "AgreementRequested"
            {
                continue;
            }

            let fields = match event.field_values() {
                Ok(f) => f,
                Err(e) => {
                    tracing::warn!("Failed to decode AgreementRequested fields: {}", e);
                    continue;
                }
            };

            let Some(provider) = field_account(&fields, "provider") else {
                tracing::warn!("AgreementRequested missing provider field");
                continue;
            };
            let provider_bytes: [u8; 32] = provider.into();
            if &provider_bytes != our_bytes {
                continue;
            }

            let Some(bucket_id) = field_u64(&fields, "bucket_id") else {
                tracing::warn!("AgreementRequested missing bucket_id field");
                continue;
            };

            tracing::info!(
                "AgreementRequested for us at block {}: bucket {}",
                block_number,
                bucket_id
            );
            self.accept(api, bucket_id).await;
        }
    }

    /// Submit `accept_agreement(bucket_id)` and wait for finalization.
    async fn accept(&self, api: &OnlineClient<PolkadotConfig>, bucket_id: u64) {
        let signer = match self.signer.as_ref() {
            Some(s) => s,
            None => {
                tracing::warn!(
                    "No signer available, skipping accept for bucket {}",
                    bucket_id
                );
                return;
            }
        };

        tracing::info!("Auto-accepting agreement for bucket {}", bucket_id);
        let tx = subxt::dynamic::tx(
            "StorageProvider",
            "accept_agreement",
            vec![DynamicValue::u128(bucket_id as u128)],
        );

        match api
            .tx()
            .sign_and_submit_then_watch_default(&tx, signer)
            .await
        {
            Ok(progress) => match progress.wait_for_finalized_success().await {
                Ok(_) => tracing::info!(
                    "Auto-accepted agreement for bucket {} (finalized)",
                    bucket_id
                ),
                Err(e) => {
                    tracing::warn!("accept_agreement tx failed for bucket {}: {}", bucket_id, e)
                }
            },
            Err(e) => tracing::warn!(
                "Failed to submit accept_agreement for bucket {}: {}",
                bucket_id,
                e
            ),
        }
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
