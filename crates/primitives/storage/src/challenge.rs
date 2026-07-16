// SPDX-License-Identifier: Apache-2.0

//! Challenge identifiers and per-challenger statistics.

use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use scale_info::TypeInfo;
use sp_core::H256;
use sp_runtime::{traits::Get, BoundedVec, MultiSignature};

use crate::{BucketId, ChunkLocation, MerkleProof, MmrProof};

/// Aggregated per-challenger statistics kept on-chain so the SDK can answer
/// "how many challenges have I issued / won / lost / earned" without scanning
/// historical events. Updated on `create_challenge`, on `ChallengeDefended`,
/// and on `ChallengeSlashed`.
#[derive(
    Clone,
    Copy,
    PartialEq,
    Eq,
    Encode,
    Decode,
    DecodeWithMemTracking,
    TypeInfo,
    MaxEncodedLen,
    Default,
    Debug,
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ChallengerStatRecord {
    /// Total challenges the challenger has ever opened.
    pub total_challenges: u32,
    /// Challenges where the provider was slashed (either invalid response or
    /// timeout). The challenger is only made whole (deposit refunded) and earns
    /// no reward — the slashed stake goes entirely to the Treasury, per the
    /// design's challenge model.
    pub successful_challenges: u32,
    /// Challenges where the provider successfully defended.
    pub failed_challenges: u32,
}

/// Active challenge against a provider.
#[derive(Clone, PartialEq, Eq, Encode, Decode, TypeInfo, MaxEncodedLen, Debug)]
pub struct Challenge<AccountId, Balance> {
    /// Bucket containing the challenged data.
    pub bucket_id: BucketId,
    /// Provider being challenged.
    pub provider: AccountId,
    /// Account that issued the challenge.
    pub challenger: AccountId,
    /// MMR root the provider committed to.
    pub mmr_root: H256,
    /// Start sequence of the commitment.
    pub start_seq: u64,
    /// Leaf + chunk being challenged.
    pub target: ChunkLocation,
    /// Deposit locked by challenger.
    pub deposit: Balance,
}

/// Challenge response from provider.
#[derive(Encode, Decode, DecodeWithMemTracking, TypeInfo)]
#[scale_info(skip_type_params(MaxChunkSize))]
pub enum ChallengeResponse<AccountId, MaxChunkSize: Get<u32>> {
    /// Provide the chunk with proofs.
    Proof {
        chunk_data: BoundedVec<u8, MaxChunkSize>,
        mmr_proof: MmrProof,
        chunk_proof: MerkleProof,
    },
    /// Data was deleted - show newer commitment without this seq.
    Deleted {
        new_mmr_root: H256,
        new_start_seq: u64,
        /// Block at which the admin signed the deletion commitment. Used
        /// as the `nonce` in `CommitmentPayload` and recency-checked by
        /// the pallet to prevent signature replay.
        nonce: u64,
        admin: AccountId,
        admin_signature: MultiSignature,
    },
    /// Challenged state has been superseded by canonical.
    Superseded,
}

// Manual impls instead of derives: std derives would put `Clone`/`PartialEq`/
// `Debug` bounds on the `MaxChunkSize` marker, which `Get<u32>` types (e.g.
// `parameter_types!` structs) do not implement.
impl<AccountId: Clone, MaxChunkSize: Get<u32>> Clone
    for ChallengeResponse<AccountId, MaxChunkSize>
{
    fn clone(&self) -> Self {
        match self {
            Self::Proof {
                chunk_data,
                mmr_proof,
                chunk_proof,
            } => Self::Proof {
                chunk_data: chunk_data.clone(),
                mmr_proof: mmr_proof.clone(),
                chunk_proof: chunk_proof.clone(),
            },
            Self::Deleted {
                new_mmr_root,
                new_start_seq,
                nonce,
                admin,
                admin_signature,
            } => Self::Deleted {
                new_mmr_root: *new_mmr_root,
                new_start_seq: *new_start_seq,
                nonce: *nonce,
                admin: admin.clone(),
                admin_signature: admin_signature.clone(),
            },
            Self::Superseded => Self::Superseded,
        }
    }
}

impl<AccountId: PartialEq, MaxChunkSize: Get<u32>> PartialEq
    for ChallengeResponse<AccountId, MaxChunkSize>
{
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::Proof {
                    chunk_data,
                    mmr_proof,
                    chunk_proof,
                },
                Self::Proof {
                    chunk_data: other_chunk_data,
                    mmr_proof: other_mmr_proof,
                    chunk_proof: other_chunk_proof,
                },
            ) => {
                chunk_data == other_chunk_data
                    && mmr_proof == other_mmr_proof
                    && chunk_proof == other_chunk_proof
            }
            (
                Self::Deleted {
                    new_mmr_root,
                    new_start_seq,
                    nonce,
                    admin,
                    admin_signature,
                },
                Self::Deleted {
                    new_mmr_root: other_new_mmr_root,
                    new_start_seq: other_new_start_seq,
                    nonce: other_nonce,
                    admin: other_admin,
                    admin_signature: other_admin_signature,
                },
            ) => {
                new_mmr_root == other_new_mmr_root
                    && new_start_seq == other_new_start_seq
                    && nonce == other_nonce
                    && admin == other_admin
                    && admin_signature == other_admin_signature
            }
            (Self::Superseded, Self::Superseded) => true,
            _ => false,
        }
    }
}

impl<AccountId: Eq, MaxChunkSize: Get<u32>> Eq for ChallengeResponse<AccountId, MaxChunkSize> {}

impl<AccountId: core::fmt::Debug, MaxChunkSize: Get<u32>> core::fmt::Debug
    for ChallengeResponse<AccountId, MaxChunkSize>
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Proof {
                chunk_data,
                mmr_proof,
                chunk_proof,
            } => f
                .debug_struct("Proof")
                .field("chunk_data", chunk_data)
                .field("mmr_proof", mmr_proof)
                .field("chunk_proof", chunk_proof)
                .finish(),
            Self::Deleted {
                new_mmr_root,
                new_start_seq,
                nonce,
                admin,
                admin_signature,
            } => f
                .debug_struct("Deleted")
                .field("new_mmr_root", new_mmr_root)
                .field("new_start_seq", new_start_seq)
                .field("nonce", nonce)
                .field("admin", admin)
                .field("admin_signature", admin_signature)
                .finish(),
            Self::Superseded => f.write_str("Superseded"),
        }
    }
}

/// Challenge identifier combining deadline and index.
#[derive(
    Clone,
    Copy,
    PartialEq,
    Eq,
    Encode,
    Decode,
    DecodeWithMemTracking,
    TypeInfo,
    MaxEncodedLen,
    Debug,
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ChallengeId<BlockNumber> {
    /// Block by which provider must respond
    pub deadline: BlockNumber,
    /// Index within the deadline's challenge list
    pub index: u16,
}
