// SPDX-License-Identifier: GPL-3.0-only

//! Decoded chain events fanned out to the background coordinators.
//!
//! The chain-state coordinator follows finalized blocks on the single chain
//! connection, decodes each block's events once, and broadcasts the
//! coordinator-relevant subset as [`BlockEvent`]s. Coordinators react to
//! events instead of polling storage maps; a slow safety-net scan (and a
//! bootstrap scan on every (re)subscribe) covers anything missed.
//!
//! Events decode through the static `storage-subxt` bindings, so a runtime
//! change that reshapes one of these events surfaces as a decode failure
//! (logged, backstopped by the safety-net scans) instead of silently
//! yielding `None` on dynamic field lookups.

use sp_runtime::AccountId32;
use storage_primitives::BucketId;
use storage_subxt::api::storage_provider::events as provider_events;
use subxt::PolkadotConfig;

/// Broadcast-channel capacity. Events per 6s block are few; coordinators
/// that lag behind this many events fall back to a bootstrap scan.
pub const EVENT_CHANNEL_CAPACITY: usize = 256;

/// Sending half of the per-block event fan-out, owned by the chain-state
/// coordinator.
pub type BlockEventTx = tokio::sync::broadcast::Sender<BlockEvent>;

/// A coordinator's subscription to the per-block event fan-out.
pub type BlockEventRx = tokio::sync::broadcast::Receiver<BlockEvent>;

/// One coordinator-relevant occurrence on the chain.
#[derive(Clone, Debug)]
pub enum BlockEvent {
    /// `StorageProvider::ChallengeCreated` — the challenge responder point-reads
    /// the full challenge at `(deadline, index)` and responds.
    ChallengeCreated {
        deadline: u32,
        index: u16,
        bucket_id: BucketId,
        provider: AccountId32,
    },
    /// `StorageProvider::ReplicaAgreementEstablished` — a new replica duty may
    /// exist for `provider`.
    ReplicaAgreementEstablished {
        bucket_id: BucketId,
        provider: AccountId32,
    },
    /// `StorageProvider::BucketCheckpointed` — a client checkpointed the
    /// bucket, so new canonical data may be available for replicas to sync.
    BucketCheckpointed { bucket_id: BucketId },
    /// The block follower (re)connected and re-read chain state wholesale.
    /// Coordinators run their bootstrap scan to catch anything missed while
    /// the stream was down. Also the correct reaction to a lagged receiver.
    Resubscribed { at_block: u32 },
}

impl From<provider_events::ChallengeCreated> for BlockEvent {
    fn from(ev: provider_events::ChallengeCreated) -> Self {
        BlockEvent::ChallengeCreated {
            deadline: ev.challenge_id.deadline,
            index: ev.challenge_id.index,
            bucket_id: ev.bucket_id,
            provider: AccountId32::new(ev.provider.0),
        }
    }
}

impl From<provider_events::ReplicaAgreementEstablished> for BlockEvent {
    fn from(ev: provider_events::ReplicaAgreementEstablished) -> Self {
        BlockEvent::ReplicaAgreementEstablished {
            bucket_id: ev.bucket_id,
            provider: AccountId32::new(ev.provider.0),
        }
    }
}

impl From<provider_events::BucketCheckpointed> for BlockEvent {
    fn from(ev: provider_events::BucketCheckpointed) -> Self {
        BlockEvent::BucketCheckpointed {
            bucket_id: ev.bucket_id,
        }
    }
}

/// Decode one block's events into the coordinator-relevant [`BlockEvent`]s.
// TODO: the try-each-variant chain below scales poorly as more events become
// coordinator-relevant; revisit a direct pallet-event -> BlockEvent mapping
// together with the dynamic-subxt vs runtime-API decision.
pub fn decode_block_events(events: &subxt::events::Events<PolkadotConfig>) -> Vec<BlockEvent> {
    events
        .iter()
        .filter_map(|event| event.ok())
        .filter_map(|event| {
            decode::<provider_events::ChallengeCreated>(&event)
                .map(BlockEvent::from)
                .or_else(|| {
                    decode::<provider_events::ReplicaAgreementEstablished>(&event)
                        .map(BlockEvent::from)
                })
                .or_else(|| {
                    decode::<provider_events::BucketCheckpointed>(&event).map(BlockEvent::from)
                })
        })
        .collect()
}

/// Statically decode `event` as `E` when its pallet/event identity matches;
/// `None` otherwise. An event that matches but fails to decode (a runtime
/// whose event shape drifted from the bindings) is logged and skipped — the
/// coordinators' safety-net scans cover the miss.
fn decode<E: subxt::events::DecodeAsEvent>(
    event: &subxt::events::Event<'_, PolkadotConfig>,
) -> Option<E> {
    match event.decode_fields_as::<E>()? {
        Ok(decoded) => Some(decoded),
        Err(e) => {
            tracing::warn!(
                "failed to decode {}::{} against the static bindings: {e}",
                event.pallet_name(),
                event.event_name(),
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use storage_subxt::api::runtime_types::storage_primitives::{
        agreement_term::AgreementTerms, ChallengeId, Commitment,
    };

    #[test]
    fn replica_agreement_established_maps_bucket_and_account() {
        let ev = provider_events::ReplicaAgreementEstablished {
            bucket_id: 11,
            provider: subxt::utils::AccountId32([4u8; 32]),
            owner: subxt::utils::AccountId32([2u8; 32]),
            terms: AgreementTerms {
                owner: subxt::utils::AccountId32([2u8; 32]),
                max_bytes: 1024,
                duration: 100,
                price_per_byte: 1,
                valid_until: 50,
                nonce: 1,
                bucket_id: Some(11),
                replica_params: None,
            },
            expires_at: 150,
        };
        let BlockEvent::ReplicaAgreementEstablished {
            bucket_id,
            provider,
        } = BlockEvent::from(ev)
        else {
            panic!("expected ReplicaAgreementEstablished");
        };
        assert_eq!(bucket_id, 11);
        assert_eq!(provider, AccountId32::new([4u8; 32]));
    }

    #[test]
    fn bucket_checkpointed_maps_bucket_id() {
        let checkpointed = provider_events::BucketCheckpointed {
            bucket_id: 22,
            commitment: Commitment {
                mmr_root: subxt::utils::H256([1u8; 32]),
                start_seq: 0,
                leaf_count: 1,
            },
            providers: vec![subxt::utils::AccountId32([4u8; 32])],
        };
        assert!(matches!(
            BlockEvent::from(checkpointed),
            BlockEvent::BucketCheckpointed { bucket_id: 22 }
        ));
    }

    #[test]
    fn challenge_created_maps_id_and_account() {
        let ev = provider_events::ChallengeCreated {
            challenge_id: ChallengeId {
                deadline: 1234,
                index: 7,
            },
            bucket_id: 42,
            provider: subxt::utils::AccountId32([5u8; 32]),
            challenger: subxt::utils::AccountId32([9u8; 32]),
            respond_by: 1234,
        };
        let BlockEvent::ChallengeCreated {
            deadline,
            index,
            bucket_id,
            provider,
        } = BlockEvent::from(ev)
        else {
            panic!("expected ChallengeCreated");
        };
        assert_eq!(deadline, 1234);
        assert_eq!(index, 7);
        assert_eq!(bucket_id, 42);
        assert_eq!(provider, AccountId32::new([5u8; 32]));
    }
}
