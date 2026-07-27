// SPDX-License-Identifier: Apache-2.0

//! Substrate client integration using subxt.
//!
//! This module provides a wrapper around subxt for interacting with
//! the storage parachain.

use crate::base::ClientError;
use crate::Signer;
use futures::StreamExt;
use sp_core::H256;
use sp_runtime::AccountId32;
use std::str::FromStr;
use storage_primitives::{ChunkLocation, Commitment};
use subxt::{OnlineClient, PolkadotConfig};

/// Substrate client for chain interactions.
#[derive(Clone)]
pub struct SubstrateClient {
    api: OnlineClient<PolkadotConfig>,
    signer: Option<Signer>,
}

impl SubstrateClient {
    /// Connect to a substrate node.
    pub async fn connect(ws_url: &str) -> Result<Self, ClientError> {
        let api = OnlineClient::<PolkadotConfig>::from_url(ws_url)
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to connect: {e}")))?;

        Ok(Self { api, signer: None })
    }

    /// Set the signer for this client (for submitting extrinsics).
    pub fn with_signer(mut self, signer: Signer) -> Self {
        self.signer = Some(signer);
        self
    }

    /// Get the API client.
    pub fn api(&self) -> &OnlineClient<PolkadotConfig> {
        &self.api
    }

    /// Get the signer if available.
    pub fn signer(&self) -> Result<&Signer, ClientError> {
        self.signer
            .as_ref()
            .ok_or_else(|| ClientError::Config("No signer configured".to_string()))
    }

    /// Parse an SS58 account ID string into AccountId32.
    pub fn parse_account(account: &str) -> Result<AccountId32, ClientError> {
        AccountId32::from_str(account)
            .map_err(|e| ClientError::Config(format!("Invalid account ID: {e}")))
    }

    /// Subscribe to finalized blocks.
    pub async fn subscribe_finalized_blocks(
        &self,
    ) -> Result<impl StreamExt<Item = Result<H256, ClientError>>, ClientError> {
        let stream = self
            .api
            .stream_blocks()
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to subscribe: {e}")))?;

        Ok(stream.map(|result| {
            result
                .map(|block| {
                    let hash = block.hash();
                    H256::from_slice(hash.as_ref())
                })
                .map_err(|e| ClientError::Chain(format!("Block stream error: {e}")))
        }))
    }
}

/// Extrinsic payload builders over the generated `storage_subxt` bindings.
///
/// Thin wrappers keeping the SDK-type → runtime-type conversions in one
/// place; the returned static payloads validate against the runtime
/// metadata at submission time.
pub mod extrinsics {
    use super::*;
    use crate::convert;
    use storage_subxt::api;
    use storage_subxt::api::runtime_types;
    use subxt::tx::Payload;

    /// Create a register_provider extrinsic payload.
    pub fn register_provider(multiaddr: Vec<u8>, public_key: Vec<u8>, stake: u128) -> impl Payload {
        api::tx().storage_provider().register_provider(
            convert::bounded(multiaddr),
            convert::bounded(public_key),
            stake,
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
        let settings = runtime_types::pallet_storage_provider::pallet::ProviderSettings {
            min_duration,
            max_duration,
            price_per_byte,
            accepting_primary,
            replica_sync_price,
            accepting_extensions,
            max_capacity,
        };
        api::tx()
            .storage_provider()
            .update_provider_settings(settings)
    }

    /// Build an `establish_storage_agreement` extrinsic payload.
    ///
    /// Bundles the SCALE-encoded provider-signed terms and signature into
    /// the dynamic call shape Layer 0 expects. The chain hashes
    /// `blake2_256(TERM_CONTEXT | SCALE(terms))` and verifies the
    /// signature against the provider's registered public key.
    pub fn establish_storage_agreement(
        provider: AccountId32,
        terms: &crate::agreement::AgreementTermsOf,
        sig: &sp_runtime::MultiSignature,
    ) -> impl Payload {
        api::tx().storage_provider().establish_storage_agreement(
            convert::account(&provider),
            convert::agreement_terms(terms),
            convert::multisig(sig),
        )
    }

    /// Create a checkpoint extrinsic payload to submit an on-chain snapshot.
    ///
    /// Fails if any signature is not exactly 64 bytes (sr25519).
    pub fn checkpoint(
        bucket_id: u64,
        commitment: Commitment,
        nonce: u64,
        signatures: Vec<(AccountId32, Vec<u8>)>,
    ) -> Result<impl Payload, ClientError> {
        let sigs = signatures
            .into_iter()
            .map(|(account, sig)| {
                Ok((convert::account(&account), convert::sr25519_signature(sig)?))
            })
            .collect::<Result<Vec<_>, ClientError>>()?;

        Ok(api::tx().storage_provider().checkpoint(
            bucket_id,
            convert::commitment(&commitment),
            nonce,
            convert::bounded(sigs),
        ))
    }

    /// Create a challenge_checkpoint extrinsic payload.
    pub fn challenge_checkpoint(
        bucket_id: u64,
        provider: AccountId32,
        target: ChunkLocation,
    ) -> impl Payload {
        api::tx().storage_provider().challenge_checkpoint(
            bucket_id,
            convert::account(&provider),
            convert::chunk_location(&target),
        )
    }

    /// Create a challenge_offchain extrinsic payload.
    ///
    /// Uses the provider's off-chain signature instead of an on-chain checkpoint.
    /// Fails if the signature is not exactly 64 bytes (sr25519).
    pub fn challenge_offchain(
        bucket_id: u64,
        provider: AccountId32,
        commitment: Commitment,
        target: ChunkLocation,
        nonce: u64,
        provider_signature: Vec<u8>,
    ) -> Result<impl Payload, ClientError> {
        Ok(api::tx().storage_provider().challenge_offchain(
            bucket_id,
            convert::account(&provider),
            convert::commitment(&commitment),
            convert::chunk_location(&target),
            nonce,
            convert::sr25519_signature(provider_signature)?,
        ))
    }

    /// Create an add_stake extrinsic payload.
    pub fn add_stake(amount: u128) -> impl Payload {
        api::tx().storage_provider().add_stake(amount)
    }

    /// Create a deregister_provider extrinsic payload.
    pub fn deregister_provider() -> impl Payload {
        api::tx().storage_provider().deregister_provider()
    }

    /// Create a confirm_replica_sync extrinsic payload.
    ///
    /// Fails if the signature is not exactly 64 bytes (sr25519).
    pub fn confirm_replica_sync(
        bucket_id: u64,
        roots: [Option<H256>; 7],
        signature: Vec<u8>,
    ) -> Result<impl Payload, ClientError> {
        Ok(api::tx().storage_provider().confirm_replica_sync(
            bucket_id,
            roots,
            convert::sr25519_signature(signature)?,
        ))
    }

    /// Create a challenge_replica extrinsic payload.
    pub fn challenge_replica(
        bucket_id: u64,
        provider: AccountId32,
        target: ChunkLocation,
    ) -> impl Payload {
        api::tx().storage_provider().challenge_replica(
            bucket_id,
            convert::account(&provider),
            convert::chunk_location(&target),
        )
    }

    /// Create a set_member extrinsic payload (add or update a bucket member's role).
    pub fn set_member(
        bucket_id: u64,
        member: AccountId32,
        role: storage_primitives::Role,
    ) -> impl Payload {
        api::tx().storage_provider().set_member(
            bucket_id,
            convert::account(&member),
            convert::role(role),
        )
    }

    /// Create a remove_member extrinsic payload.
    pub fn remove_bucket_member(bucket_id: u64, member: AccountId32) -> impl Payload {
        api::tx()
            .storage_provider()
            .remove_member(bucket_id, convert::account(&member))
    }

    /// Create a freeze_bucket extrinsic payload.
    ///
    /// The chain uses the current snapshot's start_seq to set the freeze point.
    pub fn freeze_bucket(bucket_id: u64) -> impl Payload {
        api::tx().storage_provider().freeze_bucket(bucket_id)
    }

    /// Create an extend_agreement extrinsic payload.
    pub fn extend_agreement(
        bucket_id: u64,
        provider: AccountId32,
        additional_duration: u32,
        max_payment: u128,
    ) -> impl Payload {
        api::tx().storage_provider().extend_agreement(
            bucket_id,
            convert::account(&provider),
            additional_duration,
            max_payment,
        )
    }

    /// Create a top_up_agreement extrinsic payload.
    pub fn top_up_agreement(
        bucket_id: u64,
        provider: AccountId32,
        additional_bytes: u64,
        max_payment: u128,
    ) -> impl Payload {
        api::tx().storage_provider().top_up_agreement(
            bucket_id,
            convert::account(&provider),
            additional_bytes,
            max_payment,
        )
    }

    /// Create an end_agreement extrinsic payload.
    pub fn end_agreement(
        bucket_id: u64,
        provider: AccountId32,
        action: storage_primitives::EndAction,
    ) -> impl Payload {
        api::tx().storage_provider().end_agreement(
            bucket_id,
            convert::account(&provider),
            convert::end_action(action),
        )
    }

    /// Create a set_extensions_blocked extrinsic payload (provider-side call).
    pub fn set_extensions_blocked(bucket_id: u64, blocked: bool) -> impl Payload {
        api::tx()
            .storage_provider()
            .set_extensions_blocked(bucket_id, blocked)
    }

    /// Create a respond_to_challenge extrinsic payload with a Proof response.
    pub fn respond_to_challenge_proof(
        challenge_id: (u32, u16),
        chunk_data: &[u8],
        mmr_proof: &storage_primitives::MmrProof,
        chunk_proof: &storage_primitives::MerkleProof,
    ) -> impl Payload {
        let response = runtime_types::pallet_storage_provider::pallet::ChallengeResponse::Proof {
            chunk_data: convert::bounded(chunk_data.to_vec()),
            mmr_proof: convert::mmr_proof(mmr_proof),
            chunk_proof: convert::merkle_proof(chunk_proof),
        };

        api::tx().storage_provider().respond_to_challenge(
            convert::challenge_id(challenge_id.0, challenge_id.1),
            response,
        )
    }
}

// Helper functions for common operations

/// The pallet's anchor block — the clock every on-chain duration is measured
/// against — via the `StorageProviderApi::current_anchor_block` runtime API.
/// Compare anchor-denominated values (deadlines, expiries, `checkpoint_block`)
/// against this, never the parachain height.
pub async fn fetch_current_anchor_block<C>(
    at: &subxt::client::ClientAtBlock<PolkadotConfig, C>,
) -> Result<u32, ClientError>
where
    C: subxt::client::OnlineClientAtBlockT<PolkadotConfig>,
{
    let payload = storage_subxt::api::runtime_apis()
        .storage_provider_api()
        .current_anchor_block();
    at.runtime_apis().call(payload).await.map_err(|e| {
        ClientError::Chain(format!("current_anchor_block runtime API call failed: {e}"))
    })
}

/// Parse a hex string to H256.
pub fn parse_h256(hex: &str) -> Result<H256, ClientError> {
    let hex = hex.strip_prefix("0x").unwrap_or(hex);
    let bytes =
        hex::decode(hex).map_err(|e| ClientError::Serialization(format!("Invalid hex: {e}")))?;
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
