// SPDX-License-Identifier: Apache-2.0

//! Conversions between SDK-side substrate types (`sp_runtime`,
//! `storage_primitives`) and the generated `storage_subxt` runtime types.
//!
//! Free functions rather than `From` impls because both sides are foreign
//! types (orphan rule). `sp_core::H256` needs no conversion — it is the same
//! `primitive_types::H256` that subxt re-exports.

use codec::{Decode, Encode};
use storage_subxt::api::runtime_types;

use crate::agreement::AgreementTermsOf;
use crate::base::ClientError;
use runtime_types::bounded_collections::bounded_vec::BoundedVec;
use runtime_types::storage_primitives as rt;

/// `sp_runtime::AccountId32` → `subxt::utils::AccountId32` (both wrap `[u8; 32]`).
pub fn account(a: &sp_runtime::AccountId32) -> subxt::utils::AccountId32 {
    subxt::utils::AccountId32(*AsRef::<[u8; 32]>::as_ref(a))
}

/// `subxt::utils::AccountId32` → `sp_runtime::AccountId32`.
pub fn account_back(a: &subxt::utils::AccountId32) -> sp_runtime::AccountId32 {
    sp_runtime::AccountId32::new(a.0)
}

/// Wrap a `Vec` in the generated `BoundedVec` newtype (bounds are enforced
/// on-chain at decode time, not by this wrapper).
pub fn bounded<T>(v: Vec<T>) -> BoundedVec<T> {
    BoundedVec(v)
}

/// `sp_runtime::MultiSignature` → generated `MultiSignature` via SCALE
/// round-trip: the enums are layout-identical (variant indices 0..=3), and
/// the generated side is the only runtime type with codec derives (forced in
/// the `just subxt-codegen` recipe). Covered per-variant by unit tests below.
pub fn multisig(sig: &sp_runtime::MultiSignature) -> runtime_types::sp_runtime::MultiSignature {
    Decode::decode(&mut sig.encode().as_slice())
        .expect("sp_runtime::MultiSignature and the generated MultiSignature are SCALE-identical")
}

/// Raw sr25519 signature bytes (as served by provider HTTP endpoints) →
/// generated `MultiSignature::Sr25519`.
pub fn sr25519_signature(
    sig: Vec<u8>,
) -> Result<runtime_types::sp_runtime::MultiSignature, ClientError> {
    let bytes: [u8; 64] = sig.try_into().map_err(|v: Vec<u8>| {
        ClientError::Serialization(format!(
            "sr25519 signature must be 64 bytes, got {}",
            v.len()
        ))
    })?;
    Ok(runtime_types::sp_runtime::MultiSignature::Sr25519(bytes))
}

/// [`storage_primitives::Commitment`] → generated twin.
pub fn commitment(c: &storage_primitives::Commitment) -> rt::Commitment {
    rt::Commitment {
        mmr_root: c.mmr_root,
        start_seq: c.start_seq,
        leaf_count: c.leaf_count,
    }
}

/// [`storage_primitives::ChunkLocation`] → generated twin.
pub fn chunk_location(t: &storage_primitives::ChunkLocation) -> rt::ChunkLocation {
    rt::ChunkLocation {
        leaf_index: t.leaf_index,
        chunk_index: t.chunk_index,
    }
}

/// [`storage_primitives::Role`] → generated twin.
pub fn role(r: storage_primitives::Role) -> rt::Role {
    match r {
        storage_primitives::Role::Admin => rt::Role::Admin,
        storage_primitives::Role::Writer => rt::Role::Writer,
        storage_primitives::Role::Reader => rt::Role::Reader,
    }
}

/// Generated `Role` → [`storage_primitives::Role`].
pub fn role_back(r: &rt::Role) -> storage_primitives::Role {
    match r {
        rt::Role::Admin => storage_primitives::Role::Admin,
        rt::Role::Writer => storage_primitives::Role::Writer,
        rt::Role::Reader => storage_primitives::Role::Reader,
    }
}

/// [`storage_primitives::EndAction`] → generated twin.
pub fn end_action(a: storage_primitives::EndAction) -> rt::EndAction {
    match a {
        storage_primitives::EndAction::Pay => rt::EndAction::Pay,
        storage_primitives::EndAction::Burn { burn_percent } => {
            rt::EndAction::Burn { burn_percent }
        }
    }
}

/// Build a generated `ChallengeId` from its parts.
pub fn challenge_id(deadline: u32, index: u16) -> rt::ChallengeId<u32> {
    rt::ChallengeId { deadline, index }
}

/// [`storage_primitives::MerkleProof`] → generated twin.
pub fn merkle_proof(p: &storage_primitives::MerkleProof) -> rt::MerkleProof {
    rt::MerkleProof {
        siblings: p.siblings.clone(),
        path: p.path.clone(),
    }
}

/// [`storage_primitives::MmrProof`] → generated twin.
pub fn mmr_proof(p: &storage_primitives::MmrProof) -> rt::MmrProof {
    rt::MmrProof {
        peaks: p.peaks.clone(),
        leaf: rt::MmrLeaf {
            data_root: p.leaf.data_root,
            data_size: p.leaf.data_size,
            total_size: p.leaf.total_size,
        },
        leaf_proof: merkle_proof(&p.leaf_proof),
    }
}

/// [`AgreementTermsOf`] → generated `AgreementTerms`.
///
/// Unlike the old dynamic encoder, this includes `ReplicaTerms::sync_price`
/// (the dynamic path silently dropped it, malforming replica agreements).
pub fn agreement_terms(
    t: &AgreementTermsOf,
) -> rt::agreement_term::AgreementTerms<subxt::utils::AccountId32, u128, u32> {
    use rt::agreement_term as at;
    at::AgreementTerms {
        owner: account(&t.owner),
        max_bytes: t.max_bytes,
        duration: t.duration,
        price_per_byte: t.price_per_byte,
        valid_until: t.valid_until,
        nonce: t.nonce,
        bucket_id: t.bucket_id,
        replica_params: t.replica_params.as_ref().map(|r| at::ReplicaTerms {
            sync_balance: r.sync_balance,
            min_sync_interval: r.min_sync_interval,
            sync_price: r.sync_price,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multisig_round_trips_every_variant() {
        // (variant index, payload length): Ed25519, Sr25519, Ecdsa, Eth.
        for (idx, len) in [(0u8, 64usize), (1, 64), (2, 65), (3, 65)] {
            let mut encoded = vec![idx];
            encoded.extend(std::iter::repeat_n(0xAB, len));
            let sp_sig = sp_runtime::MultiSignature::decode(&mut encoded.as_slice())
                .expect("valid sp MultiSignature bytes");
            let converted = multisig(&sp_sig);
            assert_eq!(converted.encode(), encoded, "variant index {idx}");
        }
    }

    #[test]
    fn sr25519_signature_rejects_wrong_length() {
        assert!(sr25519_signature(vec![0u8; 63]).is_err());
        assert!(sr25519_signature(vec![0u8; 65]).is_err());
        let ok = sr25519_signature(vec![7u8; 64]).unwrap();
        assert_eq!(
            ok.encode(),
            {
                let mut e = vec![1u8];
                e.extend([7u8; 64]);
                e
            },
            "Sr25519 variant index is 1"
        );
    }

    #[test]
    fn account_round_trips() {
        let sp = sp_runtime::AccountId32::new([42u8; 32]);
        let sx = account(&sp);
        assert_eq!(sx.0, [42u8; 32]);
        assert_eq!(account_back(&sx), sp);
    }

    #[test]
    fn agreement_terms_keeps_sync_price() {
        let terms = AgreementTermsOf {
            owner: sp_runtime::AccountId32::new([1u8; 32]),
            max_bytes: 10,
            duration: 20,
            price_per_byte: 30,
            valid_until: 40,
            nonce: 50,
            bucket_id: Some(60),
            replica_params: Some(crate::agreement::ReplicaTermsOf {
                sync_balance: 70,
                min_sync_interval: 80,
                sync_price: 90,
            }),
        };
        let rt_terms = agreement_terms(&terms);
        assert_eq!(rt_terms.owner.0, [1u8; 32]);
        assert_eq!(rt_terms.max_bytes, 10);
        assert_eq!(rt_terms.bucket_id, Some(60));
        let rp = rt_terms.replica_params.expect("replica params present");
        assert_eq!(rp.sync_balance, 70);
        assert_eq!(rp.min_sync_interval, 80);
        assert_eq!(rp.sync_price, 90, "sync_price must survive the conversion");
    }
}
