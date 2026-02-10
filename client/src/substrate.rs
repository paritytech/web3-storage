//! Substrate client integration using subxt.
//!
//! This module provides a wrapper around subxt for interacting with
//! the storage parachain.

use crate::base::ClientError;
use futures::StreamExt;
use sp_core::crypto::Ss58Codec;
use sp_core::H256;
use sp_runtime::AccountId32;
use std::str::FromStr;
use std::sync::Arc;
use subxt::{OnlineClient, PolkadotConfig};
use subxt_signer::sr25519::{dev, Keypair};

/// Substrate client for chain interactions.
#[derive(Clone)]
pub struct SubstrateClient {
    api: OnlineClient<PolkadotConfig>,
    signer: Option<Arc<Keypair>>,
}

impl SubstrateClient {
    /// Connect to a substrate node.
    pub async fn connect(ws_url: &str) -> Result<Self, ClientError> {
        let api = OnlineClient::<PolkadotConfig>::from_url(ws_url)
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to connect: {}", e)))?;

        Ok(Self { api, signer: None })
    }

    /// Set the signer for this client (for submitting extrinsics).
    pub fn with_signer(mut self, signer: Keypair) -> Self {
        self.signer = Some(Arc::new(signer));
        self
    }

    /// Create a client with a development keypair (for testing).
    pub fn with_dev_signer(mut self, name: &str) -> Result<Self, ClientError> {
        let keypair = match name {
            "alice" => dev::alice(),
            "bob" => dev::bob(),
            "charlie" => dev::charlie(),
            "dave" => dev::dave(),
            "eve" => dev::eve(),
            "ferdie" => dev::ferdie(),
            _ => {
                return Err(ClientError::Config(format!(
                    "Unknown dev account: {}",
                    name
                )))
            }
        };
        self.signer = Some(Arc::new(keypair));
        Ok(self)
    }

    /// Get the API client.
    pub fn api(&self) -> &OnlineClient<PolkadotConfig> {
        &self.api
    }

    /// Get the signer if available.
    pub fn signer(&self) -> Result<&Keypair, ClientError> {
        self.signer
            .as_ref()
            .map(|s| s.as_ref())
            .ok_or_else(|| ClientError::Config("No signer configured".to_string()))
    }

    /// Parse an SS58 account ID string into AccountId32.
    pub fn parse_account(account: &str) -> Result<AccountId32, ClientError> {
        AccountId32::from_str(account)
            .map_err(|e| ClientError::Config(format!("Invalid account ID: {}", e)))
    }

    /// Subscribe to finalized blocks.
    pub async fn subscribe_finalized_blocks(
        &self,
    ) -> Result<impl StreamExt<Item = Result<H256, ClientError>>, ClientError> {
        let stream = self
            .api
            .blocks()
            .subscribe_finalized()
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to subscribe: {}", e)))?;

        Ok(stream.map(|result| {
            result
                .map(|block| {
                    let hash = block.hash();
                    H256::from_slice(hash.as_ref())
                })
                .map_err(|e| ClientError::Chain(format!("Block stream error: {}", e)))
        }))
    }
}

// Runtime metadata integration
//
// To use this client with a running node, you need to:
// 1. Start your parachain node
// 2. Generate metadata: `subxt metadata -f bytes > metadata.scale`
// 3. Enable the `runtime-metadata` feature
//
// For now, we provide a manual interface for common operations.

/// Manual extrinsic construction helpers.
///
/// In production, you would use the subxt codegen macro to generate
/// these automatically from runtime metadata.
pub mod extrinsics {
    use super::*;
    use subxt::tx::Payload;

    /// Create a register_provider extrinsic payload.
    pub fn register_provider(multiaddr: Vec<u8>, public_key: Vec<u8>, stake: u128) -> impl Payload {
        subxt::dynamic::tx(
            "StorageProvider",
            "register_provider",
            vec![
                subxt::dynamic::Value::from_bytes(multiaddr),
                subxt::dynamic::Value::from_bytes(public_key),
                subxt::dynamic::Value::u128(stake),
            ],
        )
    }

    /// Create an update_provider_settings extrinsic payload.
    ///
    /// # Parameters
    /// - `min_duration`: Minimum agreement duration
    /// - `max_duration`: Maximum agreement duration
    /// - `price_per_byte`: Price per byte per block
    /// - `accepting_primary`: Whether accepting primary agreements
    /// - `replica_sync_price`: Price for replica sync (None = not accepting replicas)
    /// - `accepting_extensions`: Whether accepting extensions
    /// - `max_capacity`: Maximum storage capacity in bytes (0 = unlimited)
    #[allow(clippy::too_many_arguments)]
    pub fn update_provider_settings(
        min_duration: u32,
        max_duration: u32,
        price_per_byte: u128,
        accepting_primary: bool,
        replica_sync_price: Option<u128>,
        accepting_extensions: bool,
        max_capacity: u64,
    ) -> impl Payload {
        let replica_price_value = match replica_sync_price {
            Some(price) => subxt::dynamic::Value::unnamed_variant("Some", vec![
                subxt::dynamic::Value::u128(price),
            ]),
            None => subxt::dynamic::Value::unnamed_variant("None", vec![]),
        };

        subxt::dynamic::tx(
            "StorageProvider",
            "update_provider_settings",
            vec![subxt::dynamic::Value::named_composite([
                ("min_duration", subxt::dynamic::Value::u128(min_duration as u128)),
                ("max_duration", subxt::dynamic::Value::u128(max_duration as u128)),
                ("price_per_byte", subxt::dynamic::Value::u128(price_per_byte)),
                ("accepting_primary", subxt::dynamic::Value::bool(accepting_primary)),
                ("replica_sync_price", replica_price_value),
                ("accepting_extensions", subxt::dynamic::Value::bool(accepting_extensions)),
                ("max_capacity", subxt::dynamic::Value::u128(max_capacity as u128)),
            ])],
        )
    }

    /// Create an accept_agreement extrinsic payload.
    pub fn accept_agreement(bucket_id: u64) -> impl Payload {
        subxt::dynamic::tx(
            "StorageProvider",
            "accept_agreement",
            vec![subxt::dynamic::Value::u128(bucket_id as u128)],
        )
    }

    /// Create a create_bucket extrinsic payload.
    pub fn create_bucket(min_providers: u32) -> impl Payload {
        subxt::dynamic::tx(
            "StorageProvider",
            "create_bucket",
            vec![subxt::dynamic::Value::u128(min_providers as u128)],
        )
    }

    /// Create a request_primary_agreement extrinsic payload (admin only).
    pub fn request_primary_agreement(
        bucket_id: u64,
        provider: AccountId32,
        max_bytes: u64,
        duration: u32,
        payment: u128,
    ) -> impl Payload {
        subxt::dynamic::tx(
            "StorageProvider",
            "request_primary_agreement",
            vec![
                subxt::dynamic::Value::u128(bucket_id as u128),
                subxt::dynamic::Value::from_bytes(provider.as_ref() as &[u8]),
                subxt::dynamic::Value::u128(max_bytes as u128),
                subxt::dynamic::Value::u128(duration as u128),
                subxt::dynamic::Value::u128(payment),
            ],
        )
    }

    /// Create a request_agreement extrinsic payload (replica agreements).
    #[allow(clippy::too_many_arguments)]
    pub fn request_agreement(
        bucket_id: u64,
        provider: AccountId32,
        max_bytes: u64,
        duration: u32,
        payment: u128,
        sync_balance: u128,
        min_sync_interval: u32,
    ) -> impl Payload {
        subxt::dynamic::tx(
            "StorageProvider",
            "request_agreement",
            vec![
                subxt::dynamic::Value::u128(bucket_id as u128),
                subxt::dynamic::Value::from_bytes(provider.as_ref() as &[u8]),
                subxt::dynamic::Value::u128(max_bytes as u128),
                subxt::dynamic::Value::u128(duration as u128),
                subxt::dynamic::Value::u128(payment),
                // ReplicaRequestParams struct
                subxt::dynamic::Value::named_composite([
                    ("sync_balance", subxt::dynamic::Value::u128(sync_balance)),
                    ("min_sync_interval", subxt::dynamic::Value::u128(min_sync_interval as u128)),
                ]),
            ],
        )
    }

    /// Create a challenge_checkpoint extrinsic payload.
    pub fn challenge_checkpoint(
        bucket_id: u64,
        provider: AccountId32,
        leaf_index: u64,
        chunk_index: u64,
    ) -> impl Payload {
        subxt::dynamic::tx(
            "StorageProvider",
            "challenge_checkpoint",
            vec![
                subxt::dynamic::Value::u128(bucket_id as u128),
                subxt::dynamic::Value::from_bytes(provider.as_ref() as &[u8]),
                subxt::dynamic::Value::u128(leaf_index as u128),
                subxt::dynamic::Value::u128(chunk_index as u128),
            ],
        )
    }

    /// Create a challenge_offchain extrinsic payload.
    ///
    /// Uses the provider's off-chain signature instead of an on-chain checkpoint.
    pub fn challenge_offchain(
        bucket_id: u64,
        provider: AccountId32,
        mmr_root: H256,
        start_seq: u64,
        leaf_index: u64,
        chunk_index: u64,
        provider_signature: Vec<u8>,
    ) -> impl Payload {
        // MultiSignature::Sr25519 variant index is 0
        subxt::dynamic::tx(
            "StorageProvider",
            "challenge_offchain",
            vec![
                subxt::dynamic::Value::u128(bucket_id as u128),
                subxt::dynamic::Value::from_bytes(provider.as_ref() as &[u8]),
                subxt::dynamic::Value::from_bytes(mmr_root.as_bytes()),
                subxt::dynamic::Value::u128(start_seq as u128),
                subxt::dynamic::Value::u128(leaf_index as u128),
                subxt::dynamic::Value::u128(chunk_index as u128),
                // MultiSignature enum: Sr25519 = 0, Ed25519 = 1, Ecdsa = 2
                subxt::dynamic::Value::unnamed_variant("Sr25519", vec![
                    subxt::dynamic::Value::from_bytes(&provider_signature),
                ]),
            ],
        )
    }

    /// Create a respond_challenge extrinsic payload.
    pub fn respond_challenge(
        bucket_id: u64,
        challenge_id: (u32, u16),
        chunk_data: Vec<u8>,
        chunk_proof: Vec<H256>,
        mmr_proof: (Vec<H256>, Vec<H256>),
    ) -> impl Payload {
        subxt::dynamic::tx(
            "StorageProvider",
            "respond_challenge",
            vec![
                subxt::dynamic::Value::u128(bucket_id as u128),
                subxt::dynamic::Value::unnamed_composite(vec![
                    subxt::dynamic::Value::u128(challenge_id.0 as u128),
                    subxt::dynamic::Value::u128(challenge_id.1 as u128),
                ]),
                subxt::dynamic::Value::from_bytes(&chunk_data),
                subxt::dynamic::Value::unnamed_composite(
                    chunk_proof
                        .iter()
                        .map(|h| subxt::dynamic::Value::from_bytes(h.as_bytes()))
                        .collect::<Vec<_>>(),
                ),
                subxt::dynamic::Value::unnamed_composite(vec![
                    subxt::dynamic::Value::unnamed_composite(
                        mmr_proof
                            .0
                            .iter()
                            .map(|h| subxt::dynamic::Value::from_bytes(h.as_bytes()))
                            .collect::<Vec<_>>(),
                    ),
                    subxt::dynamic::Value::unnamed_composite(
                        mmr_proof
                            .1
                            .iter()
                            .map(|h| subxt::dynamic::Value::from_bytes(h.as_bytes()))
                            .collect::<Vec<_>>(),
                    ),
                ]),
            ],
        )
    }
}

/// Storage queries for reading chain state.
pub mod storage {
    use super::*;

    /// Query provider info.
    pub fn provider_info(
        account: &AccountId32,
    ) -> subxt::storage::DefaultAddress<
        Vec<subxt::dynamic::Value>,
        subxt::dynamic::DecodedValueThunk,
        subxt::utils::Yes,
        subxt::utils::Yes,
        subxt::utils::Yes,
    > {
        subxt::dynamic::storage(
            "StorageProvider",
            "Providers",
            vec![subxt::dynamic::Value::from_bytes(account.as_ref() as &[u8])],
        )
    }

    /// Query bucket info.
    pub fn bucket_info(
        bucket_id: u64,
    ) -> subxt::storage::DefaultAddress<
        Vec<subxt::dynamic::Value>,
        subxt::dynamic::DecodedValueThunk,
        subxt::utils::Yes,
        subxt::utils::Yes,
        subxt::utils::Yes,
    > {
        subxt::dynamic::storage(
            "StorageProvider",
            "Buckets",
            vec![subxt::dynamic::Value::u128(bucket_id as u128)],
        )
    }

    /// Query agreement info.
    pub fn agreement_info(
        bucket_id: u64,
        provider: &AccountId32,
    ) -> subxt::storage::DefaultAddress<
        Vec<subxt::dynamic::Value>,
        subxt::dynamic::DecodedValueThunk,
        subxt::utils::Yes,
        subxt::utils::Yes,
        subxt::utils::Yes,
    > {
        subxt::dynamic::storage(
            "StorageProvider",
            "StorageAgreements",
            vec![
                subxt::dynamic::Value::u128(bucket_id as u128),
                subxt::dynamic::Value::from_bytes(provider.as_ref() as &[u8]),
            ],
        )
    }
}

// Helper functions for common operations

/// Parse a hex string to H256.
pub fn parse_h256(hex: &str) -> Result<H256, ClientError> {
    let hex = hex.strip_prefix("0x").unwrap_or(hex);
    let bytes =
        hex::decode(hex).map_err(|e| ClientError::Serialization(format!("Invalid hex: {}", e)))?;
    if bytes.len() != 32 {
        return Err(ClientError::Serialization(format!(
            "Expected 32 bytes, got {}",
            bytes.len()
        )));
    }
    Ok(H256::from_slice(&bytes))
}

/// Convert H256 to hex string.
pub fn h256_to_hex(hash: &H256) -> String {
    format!("0x{}", hex::encode(hash.as_bytes()))
}
