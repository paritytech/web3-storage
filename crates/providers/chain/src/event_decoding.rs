// SPDX-License-Identifier: Apache-2.0

//! Decoding raw on-chain events into [`BlockEvent`]s.
//!
//! Events decode through the static `storage-subxt` bindings, so a runtime
//! change that reshapes one of these events surfaces as a decode failure
//! (logged, backstopped by the safety-net scans) instead of silently
//! yielding `None` on dynamic field lookups.

use crate::events::BlockEvent;
use sp_runtime::AccountId32;
use storage_subxt::api::storage_provider::events as provider_events;
use subxt::PolkadotConfig;

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
            start_seq: ev.commitment.start_seq,
        }
    }
}

impl From<provider_events::BucketCreated> for BlockEvent {
    fn from(ev: provider_events::BucketCreated) -> Self {
        BlockEvent::BucketMembershipChanged {
            bucket_id: ev.bucket_id,
        }
    }
}

impl From<provider_events::MemberSet> for BlockEvent {
    fn from(ev: provider_events::MemberSet) -> Self {
        BlockEvent::BucketMembershipChanged {
            bucket_id: ev.bucket_id,
        }
    }
}

impl From<provider_events::MemberRemoved> for BlockEvent {
    fn from(ev: provider_events::MemberRemoved) -> Self {
        BlockEvent::BucketMembershipChanged {
            bucket_id: ev.bucket_id,
        }
    }
}

impl From<provider_events::BucketDeleted> for BlockEvent {
    fn from(ev: provider_events::BucketDeleted) -> Self {
        BlockEvent::BucketDeleted {
            bucket_id: ev.bucket_id,
        }
    }
}

/// Every agreement-lifecycle event carries the same two fields and means
/// the same thing to consumers — "reconcile this bucket" — so they all
/// collapse into [`BlockEvent::AgreementChanged`].
macro_rules! agreement_changed_from {
    ($($event:ident),+ $(,)?) => {$(
        impl From<provider_events::$event> for BlockEvent {
            fn from(ev: provider_events::$event) -> Self {
                BlockEvent::AgreementChanged {
                    bucket_id: ev.bucket_id,
                    provider: AccountId32::new(ev.provider.0),
                }
            }
        }
    )+};
}

agreement_changed_from!(
    AgreementAccepted,
    StorageAgreementEstablished,
    AgreementToppedUp,
    AgreementEnded,
    AgreementExpiredClaimed,
);

/// Decode one block's events into the coordinator-relevant [`BlockEvent`]s.
///
/// A membership-changing event whose fields fail to decode is escalated
/// rather than dropped: unlike the other event kinds, silently missing one
/// means a revocation is never applied and survives for the full
/// `--auth-cache-ttl` on a node that is otherwise connected and healthy. See
/// [`decode_membership`].
// TODO: the try-each-variant chain below scales poorly as more events become
// coordinator-relevant; revisit a direct pallet-event -> BlockEvent mapping
// together with the dynamic-subxt vs runtime-API decision.
pub fn decode_block_events(
    events: &subxt::events::Events<PolkadotConfig>,
    block_number: u32,
) -> Vec<BlockEvent> {
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
                .or_else(|| {
                    decode_membership::<provider_events::BucketCreated>(&event, block_number)
                })
                .or_else(|| decode_membership::<provider_events::MemberSet>(&event, block_number))
                .or_else(|| {
                    decode_membership::<provider_events::MemberRemoved>(&event, block_number)
                })
                .or_else(|| {
                    decode_membership::<provider_events::BucketDeleted>(&event, block_number)
                })
                .or_else(|| {
                    decode::<provider_events::AgreementAccepted>(&event).map(BlockEvent::from)
                })
                .or_else(|| {
                    decode::<provider_events::StorageAgreementEstablished>(&event)
                        .map(BlockEvent::from)
                })
                .or_else(|| {
                    decode::<provider_events::AgreementToppedUp>(&event).map(BlockEvent::from)
                })
                .or_else(|| decode::<provider_events::AgreementEnded>(&event).map(BlockEvent::from))
                .or_else(|| {
                    decode::<provider_events::AgreementExpiredClaimed>(&event).map(BlockEvent::from)
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

/// Like [`decode`], but for a membership-changing event: a decode failure
/// cannot be silently dropped like the other event kinds, since there is no
/// bucket id left to invalidate and the safety net is a plain TTL, not a
/// rescan. So it escalates instead, via [`escalate_membership_decode_failure`].
fn decode_membership<E>(
    event: &subxt::events::Event<'_, PolkadotConfig>,
    block_number: u32,
) -> Option<BlockEvent>
where
    E: subxt::events::DecodeAsEvent,
    BlockEvent: From<E>,
{
    match event.decode_fields_as::<E>()? {
        Ok(decoded) => Some(BlockEvent::from(decoded)),
        Err(e) => Some(escalate_membership_decode_failure(
            event.pallet_name(),
            event.event_name(),
            block_number,
            e,
        )),
    }
}

/// A membership event that matched its pallet/event name but whose fields
/// failed to decode is treated as unknown-scope rather than dropped: which
/// bucket changed can no longer be named, so every cached member set is
/// invalidated instead, the same reaction as a lagged or restarted feed - but
/// via a variant of its own, since nothing actually reconnected and the other
/// coordinators have no reason to run their full-scan reaction to this.
fn escalate_membership_decode_failure(
    pallet: &str,
    event_name: &str,
    block_number: u32,
    err: impl std::fmt::Display,
) -> BlockEvent {
    tracing::warn!(
        "failed to decode {pallet}::{event_name} at block {block_number} against the static \
         bindings; treating as an unknown membership change: {err}"
    );
    BlockEvent::MembershipScopeUnknown {
        at_block: block_number,
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
    fn bucket_checkpointed_maps_bucket_id_and_start_seq() {
        let checkpointed = provider_events::BucketCheckpointed {
            bucket_id: 22,
            commitment: Commitment {
                mmr_root: subxt::utils::H256([1u8; 32]),
                start_seq: 5,
                leaf_count: 1,
            },
            providers: vec![subxt::utils::AccountId32([4u8; 32])],
        };
        assert!(matches!(
            BlockEvent::from(checkpointed),
            BlockEvent::BucketCheckpointed {
                bucket_id: 22,
                start_seq: 5
            }
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

    #[test]
    fn bucket_created_maps_bucket_id() {
        let ev = provider_events::BucketCreated {
            bucket_id: 9,
            admin: subxt::utils::AccountId32([4u8; 32]),
        };
        assert!(matches!(
            BlockEvent::from(ev),
            BlockEvent::BucketMembershipChanged { bucket_id: 9 }
        ));
    }

    #[test]
    fn member_set_maps_bucket_id() {
        use storage_subxt::api::runtime_types::storage_primitives::Role;

        let ev = provider_events::MemberSet {
            bucket_id: 7,
            member: subxt::utils::AccountId32([3u8; 32]),
            role: Role::Writer,
        };
        assert!(matches!(
            BlockEvent::from(ev),
            BlockEvent::BucketMembershipChanged { bucket_id: 7 }
        ));
    }

    #[test]
    fn member_removed_maps_bucket_id() {
        let ev = provider_events::MemberRemoved {
            bucket_id: 7,
            member: subxt::utils::AccountId32([3u8; 32]),
        };
        assert!(matches!(
            BlockEvent::from(ev),
            BlockEvent::BucketMembershipChanged { bucket_id: 7 }
        ));
    }

    #[test]
    fn undecodable_membership_event_escalates_to_membership_scope_unknown() {
        // A membership event has no bucket id left to invalidate once its
        // fields fail to decode, unlike every other event kind here - so it
        // must escalate to invalidating every cached bucket rather than being
        // dropped. It must not reuse `Resubscribed`: nothing reconnected, and
        // that variant also drives a full chain scan in two other
        // coordinators that have nothing to do with membership.
        let event = escalate_membership_decode_failure(
            "StorageProvider",
            "MemberSet",
            42,
            "field shape drifted from the static bindings",
        );
        assert!(matches!(
            event,
            BlockEvent::MembershipScopeUnknown { at_block: 42 }
        ));
    }
}
