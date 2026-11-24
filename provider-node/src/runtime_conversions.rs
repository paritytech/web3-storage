// SPDX-License-Identifier: GPL-3.0-only

//! Conversions between the generated runtime types and the provider's types.
//!
//! Nothing here calls the chain. [`crate::subxt_client`] makes the calls and
//! passes the values through these functions.

use crate::challenge_responder::DetectedChallenge;
use provider_replica::coordinator::{BucketSnapshot, ReplicaAgreementInfo};
use sp_core::H256;
use std::collections::HashSet;
use storage_primitives::{BucketId, ChunkLocation, Commitment};
use storage_subxt::api::runtime_types::pallet_storage_provider::types::{
    Bucket as OnChainBucket, ProviderInfo as OnChainProviderInfo, ProviderRole,
};
use storage_subxt::api::runtime_types::sp_runtime::MultiSignature as RuntimeMultiSignature;
use storage_subxt::api::runtime_types::storage_primitives::{
    mmr::{MmrLeaf as RuntimeMmrLeaf, MmrProof as RuntimeMmrProof},
    ChunkLocation as RuntimeChunkLocation, Commitment as RuntimeCommitment,
};

/// Convert an account from the `sp_runtime` representation the node uses into
/// the `subxt` one the generated bindings expect. Same 32 bytes either way.
pub(crate) fn subxt_account(who: &sp_runtime::AccountId32) -> subxt::utils::AccountId32 {
    subxt::utils::AccountId32(*<sp_runtime::AccountId32 as AsRef<[u8; 32]>>::as_ref(who))
}

/// `storage-primitives` and the generated bindings carry structurally identical
/// proof types; only the `H256` differs (`sp_core` vs `subxt::utils`).
pub(crate) fn to_runtime_hash(hash: H256) -> subxt::utils::H256 {
    subxt::utils::H256(hash.0)
}

pub(crate) fn to_runtime_mmr_proof(mmr_proof: storage_primitives::MmrProof) -> RuntimeMmrProof {
    RuntimeMmrProof {
        leaf: RuntimeMmrLeaf {
            leaf_version: mmr_proof.leaf.leaf_version,
            data_root: to_runtime_hash(mmr_proof.leaf.data_root),
            data_size: mmr_proof.leaf.data_size,
            total_size: mmr_proof.leaf.total_size,
        },
        items: mmr_proof.items.into_iter().map(to_runtime_hash).collect(),
    }
}

fn commitment(commitment: RuntimeCommitment) -> Commitment {
    Commitment {
        mmr_root: H256::from(commitment.mmr_root.0),
        start_seq: commitment.start_seq,
        leaf_count: commitment.leaf_count,
    }
}

fn chunk_location(target: RuntimeChunkLocation) -> ChunkLocation {
    ChunkLocation {
        leaf_index: target.leaf_index,
        chunk_index: target.chunk_index,
    }
}

/// Convert a `MultiSignature` into the generated runtime type. Keeps the
/// signature scheme, which the pallet checks against the registered key.
pub(crate) fn to_runtime_signature(
    signature: &sp_runtime::MultiSignature,
) -> RuntimeMultiSignature {
    use sp_runtime::MultiSignature as MS;
    match signature {
        MS::Ed25519(s) => RuntimeMultiSignature::Ed25519(s.0),
        MS::Sr25519(s) => RuntimeMultiSignature::Sr25519(s.0),
        MS::Ecdsa(s) => RuntimeMultiSignature::Ecdsa(s.0),
        MS::Eth(s) => RuntimeMultiSignature::Eth(s.0),
    }
}

/// The generated `Challenge` value held in `StorageProvider::Challenges`.
type OnChainChallenge = storage_subxt::api::storage_provider::storage::challenges::Output;

/// Map an on-chain `Challenge` at `(deadline, index)` into the responder's
/// `DetectedChallenge`, or `None` when it targets a different provider.
pub(crate) fn detected_challenge(
    deadline: u32,
    index: u16,
    challenge: OnChainChallenge,
    our_bytes: &[u8; 32],
) -> Option<DetectedChallenge> {
    if &challenge.provider.0 != our_bytes {
        return None;
    }

    Some(DetectedChallenge {
        bucket_id: challenge.bucket_id,
        deadline,
        index,
        commitment: commitment(challenge.commitment),
        target: chunk_location(challenge.target),
    })
}

/// Latest checkpoint of a bucket, or the zero snapshot when the bucket does
/// not exist or has no checkpoint.
pub(crate) fn bucket_snapshot(bucket: Option<OnChainBucket>) -> BucketSnapshot {
    match bucket.and_then(|bucket| bucket.snapshot) {
        Some(snapshot) => BucketSnapshot {
            mmr_root: H256::from(snapshot.commitment.mmr_root.0),
            leaf_count: snapshot.commitment.leaf_count,
        },
        None => BucketSnapshot {
            mmr_root: H256::zero(),
            leaf_count: 0,
        },
    }
}

/// Convert a multiaddr string to an HTTP endpoint.
fn multiaddr_to_http_endpoint(multiaddr: &str) -> String {
    let parts: Vec<&str> = multiaddr.split('/').filter(|s| !s.is_empty()).collect();

    let mut host = "127.0.0.1".to_string();
    let mut port = "3333".to_string();

    let mut i = 0;
    while i < parts.len() {
        match parts[i] {
            "ip4" | "ip6" => {
                if i + 1 < parts.len() {
                    host = parts[i + 1].to_string();
                    i += 2;
                } else {
                    i += 1;
                }
            }
            "dns4" | "dns6" | "dns" => {
                if i + 1 < parts.len() {
                    host = parts[i + 1].to_string();
                    i += 2;
                } else {
                    i += 1;
                }
            }
            "tcp" => {
                if i + 1 < parts.len() {
                    port = parts[i + 1].to_string();
                    i += 2;
                } else {
                    i += 1;
                }
            }
            _ => {
                i += 1;
            }
        }
    }

    format!("http://{host}:{port}")
}

/// Convert a bind address (e.g. "0.0.0.0:3333") to a multiaddr string
/// (e.g. "/ip4/127.0.0.1/tcp/3333").
pub(crate) fn bind_addr_to_multiaddr(bind_addr: &str) -> String {
    let parts: Vec<&str> = bind_addr.split(':').collect();
    let (host, port) = if parts.len() == 2 {
        (parts[0], parts[1])
    } else {
        ("127.0.0.1", "3333")
    };
    // 0.0.0.0 isn't useful as a client-facing address
    let host = if host == "0.0.0.0" { "127.0.0.1" } else { host };
    format!("/ip4/{host}/tcp/{port}")
}

/// HTTP endpoint a provider serves from, or `None` when it has no multiaddr.
pub(crate) fn provider_endpoint(provider: &OnChainProviderInfo) -> Option<String> {
    if provider.multiaddr.0.is_empty() {
        return None;
    }
    Some(multiaddr_to_http_endpoint(&String::from_utf8_lossy(
        &provider.multiaddr.0,
    )))
}

/// One agreement as returned by the `provider_agreements` runtime API.
type AgreementFromRuntimeApi =
    storage_subxt::api::runtime_types::pallet_storage_provider::runtime_api::AgreementResponse;

/// Narrow a `provider_agreements` entry to the replica-sync view of it, or
/// `None` for a primary agreement — the coordinator only has duties for
/// replicas.
fn replica_agreement_info(agreement: AgreementFromRuntimeApi) -> Option<ReplicaAgreementInfo> {
    let ProviderRole::Replica {
        sync_balance,
        sync_price,
        min_sync_interval,
        last_sync,
    } = agreement.role
    else {
        return None;
    };

    Some(ReplicaAgreementInfo {
        bucket_id: agreement.bucket_id,
        sync_balance,
        sync_price,
        min_sync_interval: u64::from(min_sync_interval),
        last_sync: last_sync.map(|record| {
            (
                H256::from(record.commitment.mmr_root.0),
                u64::from(record.block),
            )
        }),
    })
}

/// One challenge as returned by the `provider_challenges` runtime API.
type ChallengeFromRuntimeApi =
    storage_subxt::api::runtime_types::pallet_storage_provider::runtime_api::ChallengeResponse;

/// Map a `provider_challenges` entry into a `DetectedChallenge`. No provider
/// check here: the runtime API filtered on-chain.
pub(crate) fn detected_from_response(challenge: ChallengeFromRuntimeApi) -> DetectedChallenge {
    DetectedChallenge {
        bucket_id: challenge.bucket_id,
        deadline: challenge.deadline,
        index: challenge.index,
        commitment: commitment(challenge.commitment),
        target: chunk_location(challenge.target),
    }
}

/// The replica agreements among `agreements` whose bucket is in
/// `local_buckets`. A bucket with no local storage has nothing to sync into.
pub(crate) fn replica_agreements_for_local_buckets(
    agreements: Vec<AgreementFromRuntimeApi>,
    local_buckets: Vec<BucketId>,
) -> Vec<ReplicaAgreementInfo> {
    let local_buckets: HashSet<BucketId> = local_buckets.into_iter().collect();
    agreements
        .into_iter()
        .filter(|agreement| local_buckets.contains(&agreement.bucket_id))
        .filter_map(replica_agreement_info)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use storage_subxt::api::runtime_types::storage_primitives::{
        ChunkLocation as RuntimeChunkLocation, Commitment as RuntimeCommitment,
    };

    const OURS: [u8; 32] = [9u8; 32];

    /// Build an on-chain `Challenge` targeting `provider`.
    ///
    /// Constructed from the generated type rather than from hand-rolled SCALE
    /// bytes, so a change to the runtime's `Challenge` shape breaks the build
    /// here instead of silently shifting a byte offset at runtime.
    fn on_chain_challenge(provider: [u8; 32]) -> OnChainChallenge {
        OnChainChallenge {
            bucket_id: 42,
            provider: subxt::utils::AccountId32(provider),
            challenger: subxt::utils::AccountId32([2u8; 32]),
            commitment: RuntimeCommitment {
                mmr_root: subxt::utils::H256([3u8; 32]),
                start_seq: 100,
                leaf_count: 9,
            },
            target: RuntimeChunkLocation {
                leaf_index: 7,
                chunk_index: 5,
            },
            deposit: 1_000_000_000_000,
            authorized: false,
            created_at: 1,
        }
    }

    #[test]
    fn maps_challenge_targeting_us() {
        let detected = detected_challenge(11, 3, on_chain_challenge(OURS), &OURS)
            .expect("challenge targets us");

        assert_eq!(detected.bucket_id, 42);
        assert_eq!(detected.deadline, 11);
        assert_eq!(detected.index, 3);
        assert_eq!(detected.commitment.mmr_root.0, [3u8; 32]);
        assert_eq!(detected.commitment.leaf_count, 9);
        assert_eq!(detected.commitment.start_seq, 100);
        // Both come from the nested `target: ChunkLocation`, and they are
        // deliberately different values so a swapped mapping is caught.
        assert_eq!(detected.target.leaf_index, 7);
        assert_eq!(detected.target.chunk_index, 5);
    }

    #[test]
    fn filters_challenge_targeting_another_provider() {
        assert!(
            detected_challenge(11, 3, on_chain_challenge([1u8; 32]), &OURS).is_none(),
            "challenge for another provider is filtered out"
        );
    }

    /// Build a `provider_challenges` response entry.
    fn runtime_api_challenge() -> ChallengeFromRuntimeApi {
        ChallengeFromRuntimeApi {
            bucket_id: 42,
            provider: OURS.to_vec(),
            challenger: [2u8; 32].to_vec(),
            commitment: RuntimeCommitment {
                mmr_root: subxt::utils::H256([3u8; 32]),
                start_seq: 100,
                leaf_count: 9,
            },
            target: RuntimeChunkLocation {
                leaf_index: 7,
                chunk_index: 5,
            },
            deadline: 11,
            index: 3,
            deposit: 1_000_000_000_000,
            authorized: false,
        }
    }

    #[test]
    fn maps_runtime_api_challenge() {
        let detected = detected_from_response(runtime_api_challenge());

        assert_eq!(detected.bucket_id, 42);
        assert_eq!(detected.deadline, 11);
        assert_eq!(detected.index, 3);
        assert_eq!(detected.commitment.mmr_root.0, [3u8; 32]);
        assert_eq!(detected.commitment.leaf_count, 9);
        assert_eq!(detected.commitment.start_seq, 100);
        assert_eq!(detected.target.leaf_index, 7);
        assert_eq!(detected.target.chunk_index, 5);
    }

    /// Build an agreement response carrying `role`.
    fn runtime_api_agreement(role: ProviderRole<u128, u32>) -> AgreementFromRuntimeApi {
        AgreementFromRuntimeApi {
            bucket_id: 42,
            owner: vec![1u8; 32],
            provider: OURS.to_vec(),
            max_bytes: 1024,
            payment_locked: 10,
            price_per_byte: 2,
            expires_at: 500,
            extensions_blocked: false,
            role,
            started_at: 100,
        }
    }

    #[test]
    fn primary_agreements_are_filtered_out() {
        assert!(replica_agreement_info(runtime_api_agreement(ProviderRole::Primary)).is_none());
    }

    #[test]
    fn replica_agreement_carries_its_sync_terms_and_last_sync() {
        use storage_subxt::api::runtime_types::pallet_storage_provider::types::ReplicaSyncRecord;
        use storage_subxt::api::runtime_types::storage_primitives::Commitment;

        let info = replica_agreement_info(runtime_api_agreement(ProviderRole::Replica {
            sync_balance: 700,
            sync_price: 25,
            min_sync_interval: 600,
            last_sync: Some(ReplicaSyncRecord {
                commitment: Commitment {
                    mmr_root: subxt::utils::H256([3u8; 32]),
                    // Sits between `mmr_root` and `block`, so a mapping that
                    // walked byte offsets instead of fields would read part of
                    // this back as the block number.
                    start_seq: u64::from(u32::MAX) + 1,
                    leaf_count: 9,
                },
                block: 480,
            }),
        }))
        .expect("replica role yields sync terms");

        assert_eq!(info.bucket_id, 42);
        assert_eq!(info.sync_balance, 700);
        assert_eq!(info.sync_price, 25);
        assert_eq!(info.min_sync_interval, 600);
        assert_eq!(info.last_sync, Some((H256::from([3u8; 32]), 480)));
    }

    #[test]
    fn only_locally_stored_buckets_survive_the_local_bucket_filter() {
        let replica = || ProviderRole::Replica {
            sync_balance: 700,
            sync_price: 25,
            min_sync_interval: 600,
            last_sync: None,
        };
        let agreement_for = |bucket_id| AgreementFromRuntimeApi {
            bucket_id,
            ..runtime_api_agreement(replica())
        };

        let from_chain = vec![agreement_for(42), agreement_for(43), agreement_for(44)];

        let kept: Vec<BucketId> = replica_agreements_for_local_buckets(from_chain, vec![42, 44])
            .into_iter()
            .map(|info| info.bucket_id)
            .collect();

        assert_eq!(kept, vec![42, 44]);
    }

    #[test]
    fn replica_agreement_without_a_sync_yet_has_no_last_sync() {
        let info = replica_agreement_info(runtime_api_agreement(ProviderRole::Replica {
            sync_balance: 0,
            sync_price: 1,
            min_sync_interval: 10,
            last_sync: None,
        }))
        .expect("replica role yields sync terms");

        assert!(info.last_sync.is_none());
    }
}
