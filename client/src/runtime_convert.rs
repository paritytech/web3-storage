// SPDX-License-Identifier: Apache-2.0

//! Conversions between the client's domain types and the generated
//! `runtime_types::*` from `storage-subxt`.
//!
//! All conversions are byte-exact — they extract inner bytes directly
//! rather than going through string formatting or re-encoding.

use crate::provider_node_request_scheme::AgreementTermsOf;
use sp_core::H256;
use storage_primitives::{EndAction, MerkleProof, MmrProof, Role};
use storage_subxt::api::runtime_types as rt;
use storage_subxt::subxt::utils::AccountId32;
use storage_subxt::subxt_core::utils::H256 as RtH256;

// Convenient type aliases (for return types only, not constructors)
pub type RtMultiSig = rt::sp_runtime::MultiSignature;
pub type RtRole = rt::storage_primitives::Role;
pub type RtEndAction = rt::storage_primitives::EndAction;
pub type RtAgreementTerms =
    rt::storage_primitives::agreement_term::AgreementTerms<AccountId32, u128, u32>;
pub type RtChallengeId = rt::storage_primitives::ChallengeId<u32>;
pub type RtChallengeResponse = rt::pallet_storage_provider::pallet::ChallengeResponse;
pub type RtMmrProof = rt::storage_primitives::MmrProof;
pub type RtMerkleProof = rt::storage_primitives::MerkleProof;
pub type BoundedVec<T> = rt::bounded_collections::bounded_vec::BoundedVec<T>;

// ── Client domain → generated runtime_types ────────────────────────────────

pub fn to_h256(h: &H256) -> RtH256 {
    RtH256(h.0)
}

/// Convert raw Sr25519 bytes (64 bytes) into `MultiSignature::Sr25519`.
///
/// The checkpoint call passes raw signature slices — always Sr25519 because
/// that's what the provider node signs with.
pub fn raw_sr25519_to_multi_sig(bytes: Vec<u8>) -> RtMultiSig {
    let mut arr = [0u8; 64];
    let len = bytes.len().min(64);
    arr[..len].copy_from_slice(&bytes[..len]);
    rt::sp_runtime::MultiSignature::Sr25519(arr)
}

pub fn to_bounded_bytes(v: Vec<u8>) -> BoundedVec<u8> {
    rt::bounded_collections::bounded_vec::BoundedVec(v)
}

pub fn to_signatures(sigs: Vec<(AccountId32, Vec<u8>)>) -> BoundedVec<(AccountId32, RtMultiSig)> {
    let pairs = sigs
        .into_iter()
        .map(|(account, raw_sig)| (account, raw_sr25519_to_multi_sig(raw_sig)))
        .collect();
    rt::bounded_collections::bounded_vec::BoundedVec(pairs)
}

pub fn to_agreement_terms(terms: &AgreementTermsOf) -> RtAgreementTerms {
    let replica_params = terms.replica_params.as_ref().map(|rp| {
        rt::storage_primitives::agreement_term::ReplicaTerms {
            sync_balance: rp.sync_balance,
            min_sync_interval: rp.min_sync_interval,
            sync_price: rp.sync_price,
        }
    });
    rt::storage_primitives::agreement_term::AgreementTerms {
        owner: terms.owner.clone(),
        max_bytes: terms.max_bytes,
        duration: terms.duration,
        price_per_byte: terms.price_per_byte,
        valid_until: terms.valid_until,
        nonce: terms.nonce,
        bucket_id: terms.bucket_id,
        replica_params,
    }
}

pub fn to_role(role: Role) -> RtRole {
    match role {
        Role::Admin => rt::storage_primitives::Role::Admin,
        Role::Writer => rt::storage_primitives::Role::Writer,
        Role::Reader => rt::storage_primitives::Role::Reader,
    }
}

pub fn to_end_action(action: EndAction) -> RtEndAction {
    match action {
        EndAction::Pay => rt::storage_primitives::EndAction::Pay,
        EndAction::Burn { burn_percent } => {
            rt::storage_primitives::EndAction::Burn { burn_percent }
        }
    }
}

pub fn to_challenge_id(deadline: u32, index: u16) -> RtChallengeId {
    rt::storage_primitives::ChallengeId { deadline, index }
}

fn to_merkle_proof_inner(proof: &MerkleProof) -> RtMerkleProof {
    rt::storage_primitives::MerkleProof {
        siblings: proof.siblings.iter().map(to_h256).collect(),
        path: proof.path.clone(),
    }
}

pub fn to_challenge_response_proof(
    chunk_data: &[u8],
    mmr_proof: &MmrProof,
    chunk_proof: &MerkleProof,
) -> RtChallengeResponse {
    let rt_mmr_proof = rt::storage_primitives::MmrProof {
        peaks: mmr_proof.peaks.iter().map(to_h256).collect(),
        leaf: rt::storage_primitives::MmrLeaf {
            data_root: to_h256(&mmr_proof.leaf.data_root),
            data_size: mmr_proof.leaf.data_size,
            total_size: mmr_proof.leaf.total_size,
        },
        leaf_proof: to_merkle_proof_inner(&mmr_proof.leaf_proof),
    };
    rt::pallet_storage_provider::pallet::ChallengeResponse::Proof {
        chunk_data: rt::bounded_collections::bounded_vec::BoundedVec(chunk_data.to_vec()),
        mmr_proof: rt_mmr_proof,
        chunk_proof: to_merkle_proof_inner(chunk_proof),
    }
}

// ── Generated runtime_types → client domain ────────────────────────────────

pub fn from_h256(h: RtH256) -> H256 {
    H256::from_slice(&h.0)
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn h256_round_trip() {
        let original = H256::from([99u8; 32]);
        let rt = to_h256(&original);
        let back = from_h256(rt);
        assert_eq!(original, back);
    }
}
