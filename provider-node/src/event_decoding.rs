// SPDX-License-Identifier: GPL-3.0-only

//! Decoding a finalized block's raw events into [`BlockContents`]: the
//! [`BlockEvent`]s for the coordinators and the provider-lifecycle accounts.
//!
//! Events decode through the static `storage-subxt` bindings, so a runtime
//! change that reshapes one of these events surfaces as a decode failure
//! instead of silently yielding `None` on dynamic field lookups. See
//! [`decode_block_contents`] for how each kind of failure is handled.

use provider_coordinator::BlockContents;
use provider_events::BlockEvent;
use sp_runtime::AccountId32;
use storage_subxt::api::{
    storage_provider::{events as pallet_events, Event as StorageProviderEvent},
    system::events as system_events,
    Event,
};
use subxt::{events::Event as RawEvent, PolkadotConfig};

/// Whether `event` is a storage-provider event that changes a bucket's member
/// set or agreements. A decode failure of one of these escalates instead of being dropped;
/// see [`escalate_membership_decode_failure`].
fn is_membership_event(event: &RawEvent<'_, PolkadotConfig>) -> bool {
    event.is::<pallet_events::BucketCreated>()
        || event.is::<pallet_events::MemberSet>()
        || event.is::<pallet_events::MemberRemoved>()
        || event.is::<pallet_events::BucketDeleted>()
        || event.is::<pallet_events::BucketVisibilityChanged>()
        || event.is::<pallet_events::SlashedProviderRemoved>()
        || event.is::<pallet_events::StorageAgreementEstablished>()
        || event.is::<pallet_events::AgreementToppedUp>()
        || event.is::<pallet_events::AgreementEnded>()
        || event.is::<pallet_events::AgreementExtended>()
}

/// Whether `event` is a storage-provider event that maps to a [`BlockEvent`]
/// other than a membership change, or to a provider-lifecycle account.
pub(crate) fn is_other_relevant_event(event: &RawEvent<'_, PolkadotConfig>) -> bool {
    event.is::<pallet_events::ChallengeCreated>()
        || event.is::<pallet_events::ReplicaAgreementEstablished>()
        || event.is::<pallet_events::BucketCheckpointed>()
        || event.is::<pallet_events::ProviderRegistered>()
        || event.is::<pallet_events::ProviderSettingsUpdated>()
        || event.is::<pallet_events::ProviderMultiaddrUpdated>()
        || event.is::<pallet_events::DeregisterAnnounced>()
        || event.is::<pallet_events::DeregisterCancelled>()
        || event.is::<pallet_events::ProviderDeregistered>()
}

/// Decode the relevant events of one block into [`BlockContents`], each once;
/// other events are skipped without decoding.
///
/// A membership-changing event whose fields fail to decode is escalated
/// rather than dropped: unlike the other event kinds, silently missing one
/// means a revocation is never applied and survives for the full
/// `--auth-cache-ttl` on a node that is otherwise connected and healthy. Any
/// other relevant event that fails to decode is logged and skipped:
/// - a missed [`BlockEvent`] is covered by the coordinators' safety-net scans;
/// - a missed lifecycle account is covered by the next lifecycle event for
///   the provider, except after `ProviderDeregistered`: then only the
///   bootstrap refresh on reconnect corrects the state.
pub fn decode_block_contents(
    events: &subxt::events::Events<PolkadotConfig>,
    block_number: u32,
) -> BlockContents {
    let mut contents = BlockContents {
        events: Vec::new(),
        lifecycle: Vec::new(),
        runtime_upgraded: false,
    };
    for event in events.iter().filter_map(|event| event.ok()) {
        if event.is::<system_events::CodeUpdated>() {
            contents.runtime_upgraded = true;
            continue;
        }
        // Only events a coordinator uses are decoded; the check compares
        // names, which is cheaper than a decode.
        let membership = is_membership_event(&event);
        if !membership && !is_other_relevant_event(&event) {
            continue;
        }
        match event.decode_as::<Event>() {
            Ok(Event::StorageProvider(decoded)) => {
                contents.lifecycle.extend(lifecycle_account(&decoded));
                contents.events.extend(block_event(decoded));
            }
            Ok(_) => {}
            Err(e) if membership => contents.events.push(escalate_membership_decode_failure(
                event.pallet_name(),
                event.event_name(),
                block_number,
                e,
            )),
            Err(e) => tracing::warn!(
                "failed to decode {}::{} against the static bindings: {e}",
                event.pallet_name(),
                event.event_name(),
            ),
        }
    }
    contents
}

/// The provider account of a provider-lifecycle event, or `None` for any
/// other event.
fn lifecycle_account(event: &StorageProviderEvent) -> Option<AccountId32> {
    match event {
        StorageProviderEvent::ProviderDeregistered { provider, .. }
        | StorageProviderEvent::ProviderRegistered { provider, .. }
        | StorageProviderEvent::ProviderSettingsUpdated { provider, .. }
        | StorageProviderEvent::ProviderMultiaddrUpdated { provider, .. }
        | StorageProviderEvent::DeregisterAnnounced { provider, .. }
        | StorageProviderEvent::DeregisterCancelled { provider } => {
            Some(AccountId32::new(provider.0))
        }
        // A new arm above also needs a check in `is_other_relevant_event`;
        // without it, `decode_block_contents` skips that event.
        _ => None,
    }
}

/// The coordinator-relevant [`BlockEvent`] for a decoded storage-provider
/// event, or `None` if no coordinator reacts to it.
fn block_event(event: StorageProviderEvent) -> Option<BlockEvent> {
    match event {
        StorageProviderEvent::ChallengeCreated {
            challenge_id,
            bucket_id,
            provider,
            ..
        } => Some(BlockEvent::ChallengeCreated {
            deadline: challenge_id.deadline,
            index: challenge_id.index,
            bucket_id,
            provider: AccountId32::new(provider.0),
        }),
        StorageProviderEvent::ReplicaAgreementEstablished {
            bucket_id,
            provider,
            ..
        } => Some(BlockEvent::ReplicaAgreementEstablished {
            bucket_id,
            provider: AccountId32::new(provider.0),
        }),
        StorageProviderEvent::BucketCheckpointed { bucket_id, .. } => {
            Some(BlockEvent::BucketCheckpointed { bucket_id })
        }
        StorageProviderEvent::BucketCreated { bucket_id, .. }
        | StorageProviderEvent::MemberSet { bucket_id, .. }
        | StorageProviderEvent::MemberRemoved { bucket_id, .. }
        | StorageProviderEvent::BucketDeleted { bucket_id }
        | StorageProviderEvent::BucketVisibilityChanged { bucket_id, .. }
        | StorageProviderEvent::SlashedProviderRemoved { bucket_id, .. }
        | StorageProviderEvent::StorageAgreementEstablished { bucket_id, .. }
        | StorageProviderEvent::AgreementToppedUp { bucket_id, .. }
        | StorageProviderEvent::AgreementEnded { bucket_id, .. }
        | StorageProviderEvent::AgreementExtended { bucket_id, .. } => {
            Some(BlockEvent::BucketMembershipChanged { bucket_id })
        }
        // A new arm above also needs a check in `is_membership_event` or
        // `is_other_relevant_event`; without it, `decode_block_contents`
        // skips that event.
        _ => None,
    }
}

/// A membership event that failed to decode against the static bindings is
/// treated as unknown-scope rather than dropped: which bucket changed can no
/// longer be named, so every cached member set is invalidated instead, the
/// same reaction as a lagged or restarted feed - but via a variant of its own,
/// since nothing actually reconnected and the other coordinators have no
/// reason to run their full-scan reaction to this.
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
    use storage_subxt::api::runtime_types::pallet_storage_provider::types::ChallengeId;
    use storage_subxt::api::runtime_types::storage_primitives::{
        agreement_term::{AgreementTerms, BucketTarget},
        Commitment,
    };

    #[test]
    fn replica_agreement_established_maps_bucket_and_account() {
        let ev = StorageProviderEvent::ReplicaAgreementEstablished {
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
                bucket: BucketTarget::Existing(11),
                replica_params: None,
            },
            expires_at: 150,
        };
        let Some(BlockEvent::ReplicaAgreementEstablished {
            bucket_id,
            provider,
        }) = block_event(ev)
        else {
            panic!("expected ReplicaAgreementEstablished");
        };
        assert_eq!(bucket_id, 11);
        assert_eq!(provider, AccountId32::new([4u8; 32]));
    }

    #[test]
    fn bucket_checkpointed_maps_bucket_id() {
        let checkpointed = StorageProviderEvent::BucketCheckpointed {
            bucket_id: 22,
            commitment: Commitment {
                mmr_root: subxt::utils::H256([1u8; 32]),
                start_seq: 0,
                leaf_count: 1,
            },
            providers: vec![subxt::utils::AccountId32([4u8; 32])],
        };
        assert!(matches!(
            block_event(checkpointed),
            Some(BlockEvent::BucketCheckpointed { bucket_id: 22 })
        ));
    }

    #[test]
    fn challenge_created_maps_id_and_account() {
        let ev = StorageProviderEvent::ChallengeCreated {
            challenge_id: ChallengeId {
                deadline: 1234,
                index: 7,
            },
            bucket_id: 42,
            provider: subxt::utils::AccountId32([5u8; 32]),
            challenger: subxt::utils::AccountId32([9u8; 32]),
            respond_by: 1234,
        };
        let Some(BlockEvent::ChallengeCreated {
            deadline,
            index,
            bucket_id,
            provider,
        }) = block_event(ev)
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
        let ev = StorageProviderEvent::BucketCreated {
            bucket_id: 9,
            admin: subxt::utils::AccountId32([4u8; 32]),
        };
        assert!(matches!(
            block_event(ev),
            Some(BlockEvent::BucketMembershipChanged { bucket_id: 9 })
        ));
    }

    #[test]
    fn member_set_maps_bucket_id() {
        use storage_subxt::api::runtime_types::storage_primitives::Role;

        let ev = StorageProviderEvent::MemberSet {
            bucket_id: 7,
            member: subxt::utils::AccountId32([3u8; 32]),
            role: Role::Writer,
        };
        assert!(matches!(
            block_event(ev),
            Some(BlockEvent::BucketMembershipChanged { bucket_id: 7 })
        ));
    }

    #[test]
    fn member_removed_maps_bucket_id() {
        let ev = StorageProviderEvent::MemberRemoved {
            bucket_id: 7,
            member: subxt::utils::AccountId32([3u8; 32]),
        };
        assert!(matches!(
            block_event(ev),
            Some(BlockEvent::BucketMembershipChanged { bucket_id: 7 })
        ));
    }

    #[test]
    fn bucket_deleted_maps_bucket_id() {
        let ev = StorageProviderEvent::BucketDeleted { bucket_id: 8 };
        assert!(matches!(
            block_event(ev),
            Some(BlockEvent::BucketMembershipChanged { bucket_id: 8 })
        ));
    }

    #[test]
    fn bucket_visibility_changed_maps_bucket_id() {
        use storage_subxt::api::runtime_types::storage_primitives::Visibility;

        let ev = StorageProviderEvent::BucketVisibilityChanged {
            bucket_id: 6,
            visibility: Visibility::Private,
        };
        assert!(matches!(
            block_event(ev),
            Some(BlockEvent::BucketMembershipChanged { bucket_id: 6 })
        ));
    }

    #[test]
    fn provider_and_agreement_events_map_bucket_id() {
        let provider = || subxt::utils::AccountId32([3u8; 32]);
        let events = [
            StorageProviderEvent::SlashedProviderRemoved {
                bucket_id: 13,
                provider: provider(),
                payment_returned_to_owner: 0,
            },
            StorageProviderEvent::StorageAgreementEstablished {
                bucket_id: 14,
                provider: provider(),
                owner: subxt::utils::AccountId32([2u8; 32]),
                terms: AgreementTerms {
                    owner: subxt::utils::AccountId32([2u8; 32]),
                    max_bytes: 1024,
                    duration: 100,
                    price_per_byte: 1,
                    valid_until: 50,
                    nonce: 1,
                    bucket: BucketTarget::Existing(14),
                    replica_params: None,
                },
                expires_at: 150,
            },
            StorageProviderEvent::AgreementToppedUp {
                bucket_id: 15,
                provider: provider(),
                amount: 1,
                new_max_bytes: 2048,
            },
            StorageProviderEvent::AgreementEnded {
                bucket_id: 16,
                provider: provider(),
                payment_to_provider: 0,
                burned: 0,
            },
            StorageProviderEvent::AgreementExtended {
                bucket_id: 17,
                provider: provider(),
                new_expires_at: 300,
                payment: 1,
            },
        ];

        for (event, expected) in events.into_iter().zip(13u64..) {
            assert!(
                matches!(
                    block_event(event),
                    Some(BlockEvent::BucketMembershipChanged { bucket_id }) if bucket_id == expected
                ),
                "bucket {expected}"
            );
        }
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
