//! Subxt-based production chain client for the checkpoint coordinator.

use crate::checkpoint_coordinator::{CheckpointChainClient, CheckpointDuty};
use crate::Error;
use sp_core::crypto::Ss58Codec;
use sp_core::H256;

/// Production implementation that talks to the chain via subxt.
pub struct SubxtCheckpointChainClient {
    api: subxt::OnlineClient<subxt::PolkadotConfig>,
    signer: subxt_signer::sr25519::Keypair,
}

impl SubxtCheckpointChainClient {
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
            "Checkpoint coordinator signer: {}",
            sp_core::crypto::AccountId32::from(signer.public_key().0).to_ss58check()
        );
        tracing::info!("Checkpoint coordinator connected to {}", chain_ws_url);

        Ok(Self { api, signer })
    }
}

#[async_trait::async_trait]
impl CheckpointChainClient for SubxtCheckpointChainClient {
    async fn get_current_block(&self) -> Result<u64, Error> {
        let block = self
            .api
            .blocks()
            .at_latest()
            .await
            .map_err(|e| Error::Internal(format!("Failed to get latest block: {e}")))?;
        Ok(block.number() as u64)
    }

    async fn fetch_checkpoint_config(
        &self,
        bucket_id: storage_primitives::BucketId,
    ) -> Result<Option<(u32, u32)>, Error> {
        use subxt::dynamic::At;

        let config_query = subxt::dynamic::storage(
            "StorageProvider",
            "CheckpointConfigs",
            vec![subxt::dynamic::Value::u128(bucket_id as u128)],
        );
        let storage = self
            .api
            .storage()
            .at_latest()
            .await
            .map_err(|e| Error::Internal(format!("Failed to get storage: {e}")))?;

        match storage
            .fetch(&config_query)
            .await
            .map_err(|e| Error::Internal(format!("Failed to fetch config: {e}")))?
        {
            Some(val) => {
                let decoded = val
                    .to_value()
                    .map_err(|e| Error::Internal(format!("Failed to decode config: {e}")))?;
                let interval = decoded
                    .at("interval")
                    .and_then(|v| v.as_u128())
                    .unwrap_or(100) as u32;
                let grace_period = decoded
                    .at("grace_period")
                    .and_then(|v| v.as_u128())
                    .unwrap_or(20) as u32;
                Ok(Some((interval, grace_period)))
            }
            None => Ok(None),
        }
    }

    async fn submit_checkpoint(
        &self,
        duty: &CheckpointDuty,
        signatures: Vec<(String, String)>,
    ) -> Result<H256, Error> {
        let bucket_id = duty.bucket_id;
        let mmr_root = duty.mmr_root;
        let start_seq = duty.start_seq;
        let leaf_count = duty.leaf_count;
        let window = duty.window;

        // Build signature tuples for the extrinsic
        let mut sig_values = Vec::with_capacity(signatures.len());
        for (account, sig) in &signatures {
            let account_id: sp_core::crypto::AccountId32 =
                sp_core::crypto::Ss58Codec::from_ss58check(account).map_err(|e| {
                    Error::Internal(format!("Invalid SS58 account '{account}': {e:?}"))
                })?;
            let account_bytes: [u8; 32] = account_id.into();

            let sig_bytes = hex::decode(sig.trim_start_matches("0x"))
                .map_err(|e| Error::Internal(format!("Invalid signature hex: {e}")))?;

            sig_values.push(subxt::dynamic::Value::unnamed_composite(vec![
                subxt::dynamic::Value::from_bytes(account_bytes),
                subxt::dynamic::Value::unnamed_variant(
                    "Sr25519",
                    vec![subxt::dynamic::Value::from_bytes(sig_bytes)],
                ),
            ]));
        }

        let tx = subxt::dynamic::tx(
            "StorageProvider",
            "provider_checkpoint",
            vec![
                subxt::dynamic::Value::u128(bucket_id as u128),
                subxt::dynamic::Value::from_bytes(mmr_root.as_bytes()),
                subxt::dynamic::Value::u128(start_seq as u128),
                subxt::dynamic::Value::u128(leaf_count as u128),
                subxt::dynamic::Value::u128(window as u128),
                subxt::dynamic::Value::unnamed_composite(sig_values),
            ],
        );

        let tx_progress = self
            .api
            .tx()
            .sign_and_submit_then_watch_default(&tx, &self.signer)
            .await
            .map_err(|e| Error::Internal(format!("Failed to submit tx: {e}")))?;

        let _events = tx_progress
            .wait_for_finalized_success()
            .await
            .map_err(|e| Error::Internal(format!("Transaction failed: {e}")))?;

        Ok(H256::zero())
    }
}
