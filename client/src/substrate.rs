// SPDX-License-Identifier: Apache-2.0

//! Substrate client integration using subxt.
//!
//! This module provides a wrapper around subxt for interacting with
//! the storage parachain.

use crate::base::ClientError;
use codec::Encode;
use futures::StreamExt;
use sp_core::H256;
use sp_runtime::AccountId32;
use std::str::FromStr;
use std::sync::Arc;
use storage_primitives::{ChunkLocation, Commitment};
use subxt::{OnlineClient, PolkadotConfig};
use subxt_signer::sr25519::{dev, Keypair};

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
            PALLET_NAME,
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
            Some(price) => subxt::dynamic::Value::unnamed_variant(
                "Some",
                vec![subxt::dynamic::Value::u128(price)],
            ),
            None => subxt::dynamic::Value::unnamed_variant("None", vec![]),
        };

        subxt::dynamic::tx(
            PALLET_NAME,
            "update_provider_settings",
            vec![subxt::dynamic::Value::named_composite([
                (
                    "min_duration",
                    subxt::dynamic::Value::u128(min_duration as u128),
                ),
                (
                    "max_duration",
                    subxt::dynamic::Value::u128(max_duration as u128),
                ),
                (
                    "price_per_byte",
                    subxt::dynamic::Value::u128(price_per_byte),
                ),
                (
                    "accepting_primary",
                    subxt::dynamic::Value::bool(accepting_primary),
                ),
                ("replica_sync_price", replica_price_value),
                (
                    "accepting_extensions",
                    subxt::dynamic::Value::bool(accepting_extensions),
                ),
                (
                    "max_capacity",
                    subxt::dynamic::Value::u128(max_capacity as u128),
                ),
            ])],
        )
    }

    /// Encode an [`AgreementTermsOf`](crate::agreement::AgreementTermsOf) as
    /// a subxt dynamic value matching the on-chain composite.
    pub fn dynamic_agreement_terms(
        terms: &crate::agreement::AgreementTermsOf,
    ) -> subxt::dynamic::Value {
        let replica_params_value = match &terms.replica_params {
            None => subxt::dynamic::Value::unnamed_variant("None", vec![]),
            Some(rp) => subxt::dynamic::Value::unnamed_variant(
                "Some",
                vec![subxt::dynamic::Value::named_composite([
                    ("sync_balance", subxt::dynamic::Value::u128(rp.sync_balance)),
                    (
                        "min_sync_interval",
                        subxt::dynamic::Value::u128(rp.min_sync_interval as u128),
                    ),
                ])],
            ),
        };
        let bucket_id_value = match terms.bucket_id {
            None => subxt::dynamic::Value::unnamed_variant("None", vec![]),
            Some(id) => subxt::dynamic::Value::unnamed_variant(
                "Some",
                vec![subxt::dynamic::Value::u128(id as u128)],
            ),
        };
        subxt::dynamic::Value::named_composite([
            (
                "owner",
                subxt::dynamic::Value::from_bytes(terms.owner.as_ref() as &[u8]),
            ),
            (
                "max_bytes",
                subxt::dynamic::Value::u128(terms.max_bytes as u128),
            ),
            (
                "duration",
                subxt::dynamic::Value::u128(terms.duration as u128),
            ),
            (
                "price_per_byte",
                subxt::dynamic::Value::u128(terms.price_per_byte),
            ),
            (
                "valid_until",
                subxt::dynamic::Value::u128(terms.valid_until as u128),
            ),
            ("nonce", subxt::dynamic::Value::u128(terms.nonce as u128)),
            ("bucket_id", bucket_id_value),
            ("replica_params", replica_params_value),
        ])
    }

    /// Encode a [`sp_runtime::MultiSignature`] as a subxt dynamic variant.
    ///
    /// Variant names and payloads mirror the pallet's
    /// `verify_terms_signature` match: the signature travels as the
    /// variant's raw inner bytes.
    pub fn dynamic_multi_signature(sig: &sp_runtime::MultiSignature) -> subxt::dynamic::Value {
        let (variant, bytes) = match sig {
            sp_runtime::MultiSignature::Sr25519(s) => ("Sr25519", s.encode()),
            sp_runtime::MultiSignature::Ed25519(s) => ("Ed25519", s.encode()),
            sp_runtime::MultiSignature::Ecdsa(s) => ("Ecdsa", s.encode()),
            sp_runtime::MultiSignature::Eth(s) => ("Eth", s.encode()),
        };
        subxt::dynamic::Value::unnamed_variant(
            variant,
            vec![subxt::dynamic::Value::from_bytes(bytes)],
        )
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
        subxt::dynamic::tx(
            PALLET_NAME,
            "establish_storage_agreement",
            vec![
                subxt::dynamic::Value::from_bytes(provider.as_ref() as &[u8]),
                dynamic_agreement_terms(terms),
                dynamic_multi_signature(sig),
            ],
        )
    }

    /// Encode a [`Commitment`](storage_primitives::Commitment) as a subxt
    /// dynamic named composite (`mmr_root`, `start_seq`, `leaf_count`).
    fn dynamic_commitment(commitment: Commitment) -> subxt::dynamic::Value {
        subxt::dynamic::Value::named_composite([
            (
                "mmr_root",
                subxt::dynamic::Value::from_bytes(commitment.mmr_root.as_bytes()),
            ),
            (
                "start_seq",
                subxt::dynamic::Value::u128(commitment.start_seq as u128),
            ),
            (
                "leaf_count",
                subxt::dynamic::Value::u128(commitment.leaf_count as u128),
            ),
        ])
    }

    /// Encode a [`ChunkLocation`](storage_primitives::ChunkLocation) as a subxt
    /// dynamic named composite (`leaf_index`, `chunk_index`).
    fn dynamic_chunk_location(target: ChunkLocation) -> subxt::dynamic::Value {
        subxt::dynamic::Value::named_composite([
            (
                "leaf_index",
                subxt::dynamic::Value::u128(target.leaf_index as u128),
            ),
            (
                "chunk_index",
                subxt::dynamic::Value::u128(target.chunk_index as u128),
            ),
        ])
    }

    /// Create a checkpoint extrinsic payload to submit an on-chain snapshot.
    pub fn checkpoint(
        bucket_id: u64,
        commitment: Commitment,
        nonce: u64,
        signatures: Vec<(AccountId32, Vec<u8>)>,
    ) -> impl Payload {
        let sigs: Vec<subxt::dynamic::Value> = signatures
            .into_iter()
            .map(|(account, sig)| {
                subxt::dynamic::Value::unnamed_composite(vec![
                    subxt::dynamic::Value::from_bytes(account.as_ref() as &[u8]),
                    subxt::dynamic::Value::unnamed_variant(
                        "Sr25519",
                        vec![subxt::dynamic::Value::from_bytes(&sig)],
                    ),
                ])
            })
            .collect();

        subxt::dynamic::tx(
            PALLET_NAME,
            "checkpoint",
            vec![
                subxt::dynamic::Value::u128(bucket_id as u128),
                dynamic_commitment(commitment),
                subxt::dynamic::Value::u128(nonce as u128),
                subxt::dynamic::Value::unnamed_composite(sigs),
            ],
        )
    }

    /// Create a challenge_checkpoint extrinsic payload.
    pub fn challenge_checkpoint(
        bucket_id: u64,
        provider: AccountId32,
        target: ChunkLocation,
    ) -> impl Payload {
        subxt::dynamic::tx(
            PALLET_NAME,
            "challenge_checkpoint",
            vec![
                subxt::dynamic::Value::u128(bucket_id as u128),
                subxt::dynamic::Value::from_bytes(provider.as_ref() as &[u8]),
                dynamic_chunk_location(target),
            ],
        )
    }

    /// Create a challenge_offchain extrinsic payload.
    ///
    /// Uses the provider's off-chain signature instead of an on-chain checkpoint.
    #[allow(clippy::too_many_arguments)]
    pub fn challenge_offchain(
        bucket_id: u64,
        provider: AccountId32,
        commitment: Commitment,
        target: ChunkLocation,
        nonce: u64,
        provider_signature: Vec<u8>,
    ) -> impl Payload {
        subxt::dynamic::tx(
            PALLET_NAME,
            "challenge_offchain",
            vec![
                subxt::dynamic::Value::u128(bucket_id as u128),
                subxt::dynamic::Value::from_bytes(provider.as_ref() as &[u8]),
                dynamic_commitment(commitment),
                dynamic_chunk_location(target),
                subxt::dynamic::Value::u128(nonce as u128),
                // MultiSignature enum: Sr25519 = 0, Ed25519 = 1, Ecdsa = 2
                subxt::dynamic::Value::unnamed_variant(
                    "Sr25519",
                    vec![subxt::dynamic::Value::from_bytes(&provider_signature)],
                ),
            ],
        )
    }

    /// Create an add_stake extrinsic payload.
    pub fn add_stake(amount: u128) -> impl Payload {
        subxt::dynamic::tx(
            PALLET_NAME,
            "add_stake",
            vec![subxt::dynamic::Value::u128(amount)],
        )
    }

    /// Create a deregister_provider extrinsic payload.
    pub fn deregister_provider() -> impl Payload {
        subxt::dynamic::tx(
            PALLET_NAME,
            "deregister_provider",
            Vec::<subxt::dynamic::Value>::new(),
        )
    }

    /// Create a confirm_replica_sync extrinsic payload.
    pub fn confirm_replica_sync(
        bucket_id: u64,
        roots: [Option<H256>; 7],
        signature: Vec<u8>,
    ) -> impl Payload {
        let roots_value = subxt::dynamic::Value::unnamed_composite(
            roots
                .iter()
                .map(|r| match r {
                    Some(h) => subxt::dynamic::Value::unnamed_variant(
                        "Some",
                        vec![subxt::dynamic::Value::from_bytes(h.as_bytes())],
                    ),
                    None => subxt::dynamic::Value::unnamed_variant("None", vec![]),
                })
                .collect::<Vec<_>>(),
        );

        let sig_value = subxt::dynamic::Value::unnamed_variant(
            "Sr25519",
            vec![subxt::dynamic::Value::from_bytes(&signature)],
        );

        subxt::dynamic::tx(
            PALLET_NAME,
            "confirm_replica_sync",
            vec![
                subxt::dynamic::Value::u128(bucket_id as u128),
                roots_value,
                sig_value,
            ],
        )
    }

    /// Create a challenge_replica extrinsic payload.
    pub fn challenge_replica(
        bucket_id: u64,
        provider: AccountId32,
        target: ChunkLocation,
    ) -> impl Payload {
        subxt::dynamic::tx(
            PALLET_NAME,
            "challenge_replica",
            vec![
                subxt::dynamic::Value::u128(bucket_id as u128),
                subxt::dynamic::Value::from_bytes(provider.as_ref() as &[u8]),
                dynamic_chunk_location(target),
            ],
        )
    }

    /// Create a set_member extrinsic payload (add or update a bucket member's role).
    pub fn set_member(
        bucket_id: u64,
        member: AccountId32,
        role: storage_primitives::Role,
    ) -> impl Payload {
        let role_value = match role {
            storage_primitives::Role::Admin => {
                subxt::dynamic::Value::unnamed_variant("Admin", vec![])
            }
            storage_primitives::Role::Writer => {
                subxt::dynamic::Value::unnamed_variant("Writer", vec![])
            }
            storage_primitives::Role::Reader => {
                subxt::dynamic::Value::unnamed_variant("Reader", vec![])
            }
        };

        subxt::dynamic::tx(
            PALLET_NAME,
            "set_member",
            vec![
                subxt::dynamic::Value::u128(bucket_id as u128),
                subxt::dynamic::Value::from_bytes(member.as_ref() as &[u8]),
                role_value,
            ],
        )
    }

    /// Create a remove_member extrinsic payload.
    pub fn remove_bucket_member(bucket_id: u64, member: AccountId32) -> impl Payload {
        subxt::dynamic::tx(
            PALLET_NAME,
            "remove_member",
            vec![
                subxt::dynamic::Value::u128(bucket_id as u128),
                subxt::dynamic::Value::from_bytes(member.as_ref() as &[u8]),
            ],
        )
    }

    /// Create a freeze_bucket extrinsic payload.
    ///
    /// The chain uses the current snapshot's start_seq to set the freeze point.
    pub fn freeze_bucket(bucket_id: u64) -> impl Payload {
        subxt::dynamic::tx(
            PALLET_NAME,
            "freeze_bucket",
            vec![subxt::dynamic::Value::u128(bucket_id as u128)],
        )
    }

    /// Create an extend_agreement extrinsic payload.
    pub fn extend_agreement(
        bucket_id: u64,
        provider: AccountId32,
        additional_duration: u32,
        max_payment: u128,
    ) -> impl Payload {
        subxt::dynamic::tx(
            PALLET_NAME,
            "extend_agreement",
            vec![
                subxt::dynamic::Value::u128(bucket_id as u128),
                subxt::dynamic::Value::from_bytes(provider.as_ref() as &[u8]),
                subxt::dynamic::Value::u128(additional_duration as u128),
                subxt::dynamic::Value::u128(max_payment),
            ],
        )
    }

    /// Create a top_up_agreement extrinsic payload.
    pub fn top_up_agreement(
        bucket_id: u64,
        provider: AccountId32,
        additional_bytes: u64,
        max_payment: u128,
    ) -> impl Payload {
        subxt::dynamic::tx(
            PALLET_NAME,
            "top_up_agreement",
            vec![
                subxt::dynamic::Value::u128(bucket_id as u128),
                subxt::dynamic::Value::from_bytes(provider.as_ref() as &[u8]),
                subxt::dynamic::Value::u128(additional_bytes as u128),
                subxt::dynamic::Value::u128(max_payment),
            ],
        )
    }

    /// Create an end_agreement extrinsic payload.
    pub fn end_agreement(
        bucket_id: u64,
        provider: AccountId32,
        action: storage_primitives::EndAction,
    ) -> impl Payload {
        let action_value = match action {
            storage_primitives::EndAction::Pay => {
                subxt::dynamic::Value::unnamed_variant("Pay", vec![])
            }
            storage_primitives::EndAction::Burn { burn_percent } => {
                subxt::dynamic::Value::named_variant(
                    "Burn",
                    [(
                        "burn_percent",
                        subxt::dynamic::Value::u128(burn_percent as u128),
                    )],
                )
            }
        };

        subxt::dynamic::tx(
            PALLET_NAME,
            "end_agreement",
            vec![
                subxt::dynamic::Value::u128(bucket_id as u128),
                subxt::dynamic::Value::from_bytes(provider.as_ref() as &[u8]),
                action_value,
            ],
        )
    }

    /// Create a set_extensions_blocked extrinsic payload (provider-side call).
    pub fn set_extensions_blocked(bucket_id: u64, blocked: bool) -> impl Payload {
        subxt::dynamic::tx(
            PALLET_NAME,
            "set_extensions_blocked",
            vec![
                subxt::dynamic::Value::u128(bucket_id as u128),
                subxt::dynamic::Value::bool(blocked),
            ],
        )
    }

    /// Create a respond_to_challenge extrinsic payload with a Proof response.
    ///
    /// Builds the `ChallengeResponse::Proof` variant with proper nested types
    /// matching the pallet's expected format.
    pub fn respond_to_challenge_proof(
        challenge_id: (u32, u16),
        chunk_data: &[u8],
        mmr_proof: &storage_primitives::MmrProof,
        chunk_proof: &storage_primitives::MerkleProof,
    ) -> impl Payload {
        // Build ChallengeId named composite
        let challenge_id_value = subxt::dynamic::Value::named_composite([
            (
                "deadline",
                subxt::dynamic::Value::u128(challenge_id.0 as u128),
            ),
            ("index", subxt::dynamic::Value::u128(challenge_id.1 as u128)),
        ]);

        // Build MerkleProof for leaf_proof (MMR leaf to peak)
        let leaf_proof_value = subxt::dynamic::Value::named_composite([
            (
                "siblings",
                subxt::dynamic::Value::unnamed_composite(
                    mmr_proof
                        .leaf_proof
                        .siblings
                        .iter()
                        .map(|h| subxt::dynamic::Value::from_bytes(h.as_bytes()))
                        .collect::<Vec<_>>(),
                ),
            ),
            (
                "path",
                subxt::dynamic::Value::unnamed_composite(
                    mmr_proof
                        .leaf_proof
                        .path
                        .iter()
                        .map(|b| subxt::dynamic::Value::bool(*b))
                        .collect::<Vec<_>>(),
                ),
            ),
        ]);

        // Build MmrLeaf
        let leaf_value = subxt::dynamic::Value::named_composite([
            (
                "data_root",
                subxt::dynamic::Value::from_bytes(mmr_proof.leaf.data_root.as_bytes()),
            ),
            (
                "data_size",
                subxt::dynamic::Value::u128(mmr_proof.leaf.data_size as u128),
            ),
            (
                "total_size",
                subxt::dynamic::Value::u128(mmr_proof.leaf.total_size as u128),
            ),
        ]);

        // Build MmrProof
        let mmr_proof_value = subxt::dynamic::Value::named_composite([
            (
                "peaks",
                subxt::dynamic::Value::unnamed_composite(
                    mmr_proof
                        .peaks
                        .iter()
                        .map(|h| subxt::dynamic::Value::from_bytes(h.as_bytes()))
                        .collect::<Vec<_>>(),
                ),
            ),
            ("leaf", leaf_value),
            ("leaf_proof", leaf_proof_value),
        ]);

        // Build MerkleProof for chunk proof (chunk to data_root)
        let chunk_proof_value = subxt::dynamic::Value::named_composite([
            (
                "siblings",
                subxt::dynamic::Value::unnamed_composite(
                    chunk_proof
                        .siblings
                        .iter()
                        .map(|h| subxt::dynamic::Value::from_bytes(h.as_bytes()))
                        .collect::<Vec<_>>(),
                ),
            ),
            (
                "path",
                subxt::dynamic::Value::unnamed_composite(
                    chunk_proof
                        .path
                        .iter()
                        .map(|b| subxt::dynamic::Value::bool(*b))
                        .collect::<Vec<_>>(),
                ),
            ),
        ]);

        // Build ChallengeResponse::Proof variant
        let response = subxt::dynamic::Value::named_variant(
            "Proof",
            [
                ("chunk_data", subxt::dynamic::Value::from_bytes(chunk_data)),
                ("mmr_proof", mmr_proof_value),
                ("chunk_proof", chunk_proof_value),
            ],
        );

        subxt::dynamic::tx(
            PALLET_NAME,
            "respond_to_challenge",
            vec![challenge_id_value, response],
        )
    }
}

/// Runtime constant addresses for reading on-chain config.
pub mod constants {
    use super::*;

    /// Dynamic constant address for `StorageProvider::RequestTimeout`,
    /// the validity window (in blocks) the chain enforces on provider-signed
    /// agreement terms.
    pub fn request_timeout() -> subxt::constants::DynamicAddress<subxt::dynamic::Value> {
        subxt::dynamic::constant(PALLET_NAME, "RequestTimeout")
    }
}

/// Storage queries for reading chain state.
///
/// Keyed helpers return `(address, keys)` pairs: pass the keys to
/// `try_fetch`/`iter` at call time (subxt 0.50 moves keys out of the address).
pub mod storage {
    use super::*;

    /// Dynamic storage address with `KeyParts` keys, decoding to a dynamic `Value`.
    pub type DynamicAddress<KeyParts> =
        subxt::storage::DynamicAddress<KeyParts, subxt::dynamic::Value>;

    /// Query provider info.
    pub fn provider_info(
        account: &AccountId32,
    ) -> (
        DynamicAddress<(subxt::dynamic::Value,)>,
        (subxt::dynamic::Value,),
    ) {
        (
            subxt::dynamic::storage(PALLET_NAME, "Providers"),
            (subxt::dynamic::Value::from_bytes(account.as_ref() as &[u8]),),
        )
    }

    /// Query bucket info.
    pub fn bucket_info(
        bucket_id: u64,
    ) -> (
        DynamicAddress<(subxt::dynamic::Value,)>,
        (subxt::dynamic::Value,),
    ) {
        (
            subxt::dynamic::storage(PALLET_NAME, "Buckets"),
            (subxt::dynamic::Value::u128(bucket_id as u128),),
        )
    }

    /// Query agreement info.
    pub fn agreement_info(
        bucket_id: u64,
        provider: &AccountId32,
    ) -> (
        DynamicAddress<(subxt::dynamic::Value, subxt::dynamic::Value)>,
        (subxt::dynamic::Value, subxt::dynamic::Value),
    ) {
        (
            subxt::dynamic::storage(PALLET_NAME, "StorageAgreements"),
            (
                subxt::dynamic::Value::u128(bucket_id as u128),
                subxt::dynamic::Value::from_bytes(provider.as_ref() as &[u8]),
            ),
        )
    }

    /// Query all storage agreements for a specific bucket (prefix iteration).
    ///
    /// Key layout after prefix: [blake2_128(provider)=16][provider=32]; provider at offset 72 in full key.
    pub fn agreements_for_bucket(
        bucket_id: u64,
    ) -> (
        DynamicAddress<(subxt::dynamic::Value, subxt::dynamic::Value)>,
        (subxt::dynamic::Value,),
    ) {
        (
            subxt::dynamic::storage(PALLET_NAME, "StorageAgreements"),
            (subxt::dynamic::Value::u128(bucket_id as u128),),
        )
    }

    /// Query the list of bucket IDs that an account is a member of.
    pub fn member_buckets(
        account: &AccountId32,
    ) -> (
        DynamicAddress<(subxt::dynamic::Value,)>,
        (subxt::dynamic::Value,),
    ) {
        (
            subxt::dynamic::storage(PALLET_NAME, "MemberBuckets"),
            (subxt::dynamic::Value::from_bytes(account.as_ref() as &[u8]),),
        )
    }

    /// Iterate all registered providers.
    ///
    /// Key layout: [twox128(pallet)=16][twox128(storage)=16][blake2_128(account)=16][account=32];
    /// account at [48..80].
    pub fn all_providers() -> DynamicAddress<(subxt::dynamic::Value,)> {
        subxt::dynamic::storage(PALLET_NAME, "Providers")
    }

    /// Iterate all storage agreements (bucket_id × provider DoubleMap).
    ///
    /// Key layout: [twox128(pallet)=16][twox128(storage)=16][blake2_128(bucket_id)=16][bucket_id=8]
    ///             [blake2_128(provider)=16][provider=32]; bucket_id at [48..56], provider at [72..104].
    pub fn all_storage_agreements() -> DynamicAddress<(subxt::dynamic::Value, subxt::dynamic::Value)>
    {
        subxt::dynamic::storage(PALLET_NAME, "StorageAgreements")
    }

    /// Iterate all active challenge entries (deadline_block → Vec<Challenge>).
    ///
    /// Key layout: [twox128(pallet)=16][twox128(storage)=16][blake2_128(block)=16][block=4];
    /// deadline block at [48..52].
    pub fn all_challenges() -> DynamicAddress<(subxt::dynamic::Value,)> {
        subxt::dynamic::storage(PALLET_NAME, "Challenges")
    }

    /// Query challenges at a deadline block.
    pub fn challenges(
        deadline_block: u32,
    ) -> (
        DynamicAddress<(subxt::dynamic::Value,)>,
        (subxt::dynamic::Value,),
    ) {
        (
            subxt::dynamic::storage(PALLET_NAME, "Challenges"),
            (subxt::dynamic::Value::u128(deadline_block as u128),),
        )
    }

    /// Query the provider's replay-window state.
    pub fn provider_replay_state(
        account: &AccountId32,
    ) -> (
        DynamicAddress<(subxt::dynamic::Value,)>,
        (subxt::dynamic::Value,),
    ) {
        (
            subxt::dynamic::storage(PALLET_NAME, "ProviderReplayStates"),
            (subxt::dynamic::Value::from_bytes(account.as_ref() as &[u8]),),
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
    use codec::Decode;
    // Raw `state_call` + manual decode so the API needn't be in the node's
    // metadata snapshot.
    let bytes = at
        .runtime_apis()
        .call_raw("StorageProviderApi_current_anchor_block", None)
        .await
        .map_err(|e| {
            ClientError::Chain(format!("current_anchor_block runtime API call failed: {e}"))
        })?;
    u32::decode(&mut &bytes[..])
        .map_err(|e| ClientError::Chain(format!("Failed to decode anchor block: {e}")))
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
