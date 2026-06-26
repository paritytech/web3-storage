// SPDX-License-Identifier: Apache-2.0

//! Substrate client integration using subxt.
//!
//! This module provides a wrapper around subxt for interacting with
//! the storage parachain.

use crate::base::ClientError;
use futures::StreamExt;
use sp_core::H256;
use sp_runtime::AccountId32;
use std::str::FromStr;
use std::sync::Arc;
use storage_subxt::storage_paseo_runtime::api::runtime_types::pallet_storage_provider::pallet::ProviderSettings;
use storage_subxt::subxt::{OnlineClient, PolkadotConfig};
use storage_subxt::subxt_signer::sr25519::{dev, Keypair};

pub const PALLET_NAME: &str = "StorageProvider";

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
            .map_err(|e| ClientError::Chain(format!("Failed to connect: {e}")))?;

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
            _ => return Err(ClientError::Config(format!("Unknown dev account: {name}"))),
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
            .map_err(|e| ClientError::Config(format!("Invalid account ID: {e}")))
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
    use crate::runtime_convert as rc;
    use storage_subxt::storage_runtime::api as runtime;
    use storage_subxt::subxt::tx::Payload;

    pub fn register_provider(multiaddr: Vec<u8>, public_key: Vec<u8>, stake: u128) -> impl Payload {
        runtime::tx().storage_provider().register_provider(
            rc::to_bounded_bytes(multiaddr),
            rc::to_bounded_bytes(public_key),
            stake,
        )
    }

    pub fn update_provider_settings(settings: ProviderSettings) -> impl Payload {
        runtime::tx()
            .storage_provider()
            .update_provider_settings(settings)
    }

    pub fn establish_storage_agreement(
        provider: AccountId32,
        terms: &crate::agreement::AgreementTermsOf,
        sig: &sp_runtime::MultiSignature,
    ) -> impl Payload {
        runtime::tx()
            .storage_provider()
            .establish_storage_agreement(
                rc::to_account(&provider),
                rc::to_agreement_terms(terms),
                rc::to_multi_sig(sig),
            )
    }

    pub fn checkpoint(
        bucket_id: u64,
        mmr_root: H256,
        start_seq: u64,
        leaf_count: u64,
        signatures: Vec<(AccountId32, Vec<u8>)>,
    ) -> impl Payload {
        runtime::tx().storage_provider().checkpoint(
            bucket_id,
            rc::to_h256(&mmr_root),
            start_seq,
            leaf_count,
            rc::to_signatures(signatures),
        )
    }

    pub fn challenge_checkpoint(
        bucket_id: u64,
        provider: AccountId32,
        leaf_index: u64,
        chunk_index: u64,
    ) -> impl Payload {
        runtime::tx().storage_provider().challenge_checkpoint(
            bucket_id,
            rc::to_account(&provider),
            leaf_index,
            chunk_index,
        )
    }

    pub fn challenge_offchain(
        bucket_id: u64,
        provider: AccountId32,
        mmr_root: H256,
        start_seq: u64,
        leaf_index: u64,
        chunk_index: u64,
        provider_signature: Vec<u8>,
    ) -> impl Payload {
        runtime::tx().storage_provider().challenge_offchain(
            bucket_id,
            rc::to_account(&provider),
            rc::to_h256(&mmr_root),
            start_seq,
            leaf_index,
            chunk_index,
            rc::raw_sr25519_to_multi_sig(provider_signature),
        )
    }

    pub fn add_stake(amount: u128) -> impl Payload {
        runtime::tx().storage_provider().add_stake(amount)
    }

    pub fn deregister_provider() -> impl Payload {
        runtime::tx().storage_provider().deregister_provider()
    }

    pub fn confirm_replica_sync(
        bucket_id: u64,
        roots: [Option<H256>; 7],
        signature: Vec<u8>,
    ) -> impl Payload {
        let rt_roots = roots.map(|r| r.as_ref().map(rc::to_h256));
        runtime::tx().storage_provider().confirm_replica_sync(
            bucket_id,
            rt_roots,
            rc::raw_sr25519_to_multi_sig(signature),
        )
    }

    pub fn challenge_replica(
        bucket_id: u64,
        provider: AccountId32,
        leaf_index: u64,
        chunk_index: u64,
    ) -> impl Payload {
        runtime::tx().storage_provider().challenge_replica(
            bucket_id,
            rc::to_account(&provider),
            leaf_index,
            chunk_index,
        )
    }

    pub fn set_member(
        bucket_id: u64,
        member: AccountId32,
        role: storage_primitives::Role,
    ) -> impl Payload {
        runtime::tx().storage_provider().set_member(
            bucket_id,
            rc::to_account(&member),
            rc::to_role(role),
        )
    }

    pub fn remove_bucket_member(bucket_id: u64, member: AccountId32) -> impl Payload {
        runtime::tx()
            .storage_provider()
            .remove_member(bucket_id, rc::to_account(&member))
    }

    pub fn freeze_bucket(bucket_id: u64) -> impl Payload {
        runtime::tx().storage_provider().freeze_bucket(bucket_id)
    }

    pub fn extend_agreement(
        bucket_id: u64,
        provider: AccountId32,
        additional_duration: u32,
        max_payment: u128,
    ) -> impl Payload {
        runtime::tx().storage_provider().extend_agreement(
            bucket_id,
            rc::to_account(&provider),
            additional_duration,
            max_payment,
        )
    }

    pub fn top_up_agreement(
        bucket_id: u64,
        provider: AccountId32,
        additional_bytes: u64,
        max_payment: u128,
    ) -> impl Payload {
        runtime::tx().storage_provider().top_up_agreement(
            bucket_id,
            rc::to_account(&provider),
            additional_bytes,
            max_payment,
        )
    }

    pub fn end_agreement(
        bucket_id: u64,
        provider: AccountId32,
        action: storage_primitives::EndAction,
    ) -> impl Payload {
        runtime::tx().storage_provider().end_agreement(
            bucket_id,
            rc::to_account(&provider),
            rc::to_end_action(action),
        )
    }

    pub fn set_extensions_blocked(bucket_id: u64, blocked: bool) -> impl Payload {
        runtime::tx()
            .storage_provider()
            .set_extensions_blocked(bucket_id, blocked)
    }

    pub fn respond_to_challenge_proof(
        challenge_id: (u32, u16),
        chunk_data: &[u8],
        mmr_proof: &storage_primitives::MmrProof,
        chunk_proof: &storage_primitives::MerkleProof,
    ) -> impl Payload {
        runtime::tx().storage_provider().respond_to_challenge(
            rc::to_challenge_id(challenge_id.0, challenge_id.1),
            rc::to_challenge_response_proof(chunk_data, mmr_proof, chunk_proof),
        )
    }

    pub fn update_provider_multiaddr(multiaddr: Vec<u8>) -> impl Payload {
        runtime::tx()
            .storage_provider()
            .update_provider_multiaddr(rc::to_bounded_bytes(multiaddr))
    }

    pub fn provider_checkpoint(
        bucket_id: u64,
        mmr_root: H256,
        start_seq: u64,
        leaf_count: u64,
        window: u64,
        signatures: Vec<(AccountId32, Vec<u8>)>,
    ) -> impl Payload {
        runtime::tx().storage_provider().provider_checkpoint(
            bucket_id,
            rc::to_h256(&mmr_root),
            start_seq,
            leaf_count,
            window,
            rc::to_signatures(signatures),
        )
    }
}

/// Runtime constant addresses for reading on-chain config.
pub mod constants {
    use storage_subxt::storage_runtime::api as runtime;
    use storage_subxt::subxt_core::constants::address::StaticAddress;

    /// Typed constant address for `StorageProvider::RequestTimeout`.
    pub fn request_timeout() -> StaticAddress<u32> {
        runtime::constants().storage_provider().request_timeout()
    }
}

/// Storage queries for reading chain state.
pub mod storage {
    use crate::runtime_convert as rc;
    use sp_runtime::AccountId32;
    use storage_subxt::storage_runtime::api as runtime;
    use storage_subxt::storage_runtime::api::runtime_types as rt;
    use storage_subxt::subxt;

    pub fn provider_info(
        account: &AccountId32,
    ) -> impl subxt::storage::Address<
        IsFetchable = subxt::utils::Yes,
        Target = rt::pallet_storage_provider::pallet::ProviderInfo,
    > {
        runtime::storage()
            .storage_provider()
            .providers(rc::to_account(account))
    }

    pub fn bucket_info(
        bucket_id: u64,
    ) -> impl subxt::storage::Address<
        IsFetchable = subxt::utils::Yes,
        Target = rt::pallet_storage_provider::pallet::Bucket,
    > {
        runtime::storage().storage_provider().buckets(bucket_id)
    }

    pub fn agreement_info(
        bucket_id: u64,
        provider: &AccountId32,
    ) -> impl subxt::storage::Address<
        IsFetchable = subxt::utils::Yes,
        Target = rt::pallet_storage_provider::pallet::StorageAgreement,
    > {
        runtime::storage()
            .storage_provider()
            .storage_agreements(bucket_id, rc::to_account(provider))
    }

    pub fn agreements_for_bucket(
        bucket_id: u64,
    ) -> impl subxt::storage::Address<
        IsIterable = subxt::utils::Yes,
        Target = rt::pallet_storage_provider::pallet::StorageAgreement,
    > {
        runtime::storage()
            .storage_provider()
            .storage_agreements_iter1(bucket_id)
    }

    pub fn member_buckets(
        account: &AccountId32,
    ) -> impl subxt::storage::Address<
        IsFetchable = subxt::utils::Yes,
        Target = rt::bounded_collections::bounded_vec::BoundedVec<u64>,
    > {
        runtime::storage()
            .storage_provider()
            .member_buckets(rc::to_account(account))
    }

    pub fn all_providers() -> impl subxt::storage::Address<
        IsIterable = subxt::utils::Yes,
        Target = rt::pallet_storage_provider::pallet::ProviderInfo,
    > {
        runtime::storage().storage_provider().providers_iter()
    }

    pub fn all_storage_agreements() -> impl subxt::storage::Address<
        IsIterable = subxt::utils::Yes,
        Target = rt::pallet_storage_provider::pallet::StorageAgreement,
    > {
        runtime::storage()
            .storage_provider()
            .storage_agreements_iter()
    }

    pub fn all_challenges() -> impl subxt::storage::Address<
        IsIterable = subxt::utils::Yes,
        Target = Vec<rt::pallet_storage_provider::pallet::Challenge>,
    > {
        runtime::storage().storage_provider().challenges_iter()
    }

    pub fn challenges(
        deadline_block: u32,
    ) -> impl subxt::storage::Address<
        IsFetchable = subxt::utils::Yes,
        Target = Vec<rt::pallet_storage_provider::pallet::Challenge>,
    > {
        runtime::storage()
            .storage_provider()
            .challenges(deadline_block)
    }

    pub fn provider_replay_state(
        account: &AccountId32,
    ) -> impl subxt::storage::Address<
        IsFetchable = subxt::utils::Yes,
        Target = rt::storage_primitives::provider_replay_state::ReplayWindow,
    > {
        runtime::storage()
            .storage_provider()
            .provider_replay_states(rc::to_account(account))
    }

    pub fn iter_providers_typed() -> impl subxt::storage::Address<
        IsIterable = subxt::utils::Yes,
        Target = rt::pallet_storage_provider::pallet::ProviderInfo,
    > {
        runtime::storage().storage_provider().providers_iter()
    }

    pub fn checkpoint_config(
        bucket_id: u64,
    ) -> impl subxt::storage::Address<
        IsFetchable = subxt::utils::Yes,
        Target = rt::storage_primitives::CheckpointWindowConfig<u32>,
    > {
        runtime::storage()
            .storage_provider()
            .checkpoint_configs(bucket_id)
    }
}

// Helper functions for common operations

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
