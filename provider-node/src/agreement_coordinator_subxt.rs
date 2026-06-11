//! Subxt-based production chain client for the agreement coordinator.

use crate::agreement_coordinator::AgreementChainClient;
use crate::Error;
use storage_primitives::BucketId;
use subxt::dynamic::Value;

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

#[async_trait::async_trait]
impl AgreementChainClient for SubxtAgreementChainClient {
    async fn fetch_pending_requests(
        &self,
        provider_account: &[u8; 32],
    ) -> Result<Vec<BucketId>, Error> {
        let our_bytes = *provider_account;
        {
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
                let bucket_id = match key_bytes[48..56].try_into() {
                    Ok(bytes) => u64::from_le_bytes(bytes),
                    Err(_) => {
                        tracing::warn!("Failed to parse bucket ID from key bytes, skipping");
                        continue;
                    }
                };

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
        }
    }

    async fn accept_agreement(&self, bucket_id: BucketId) -> Result<(), Error> {
        let tx = subxt::dynamic::tx(
            "StorageProvider",
            "accept_agreement",
            vec![Value::u128(bucket_id as u128)],
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
    }
}
