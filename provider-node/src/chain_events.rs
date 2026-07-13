// SPDX-License-Identifier: GPL-3.0-only

//! Decoded chain events fanned out to the background coordinators.
//!
//! The chain-state coordinator follows finalized blocks on the single chain
//! connection, decodes each block's events once, and broadcasts the
//! coordinator-relevant subset as [`BlockEvent`]s. Coordinators react to
//! events instead of polling storage maps; a slow safety-net scan (and a
//! bootstrap scan on every (re)subscribe) covers anything missed.

use sp_runtime::AccountId32;
use storage_primitives::BucketId;
use subxt::ext::scale_value::{At, Value};
use subxt::PolkadotConfig;

/// Broadcast-channel capacity. Events per 6s block are few; coordinators
/// that lag behind this many events fall back to a bootstrap scan.
pub const EVENT_CHANNEL_CAPACITY: usize = 256;

/// One coordinator-relevant occurrence on the chain.
#[derive(Clone, Debug)]
pub enum BlockEvent {
    /// A finalized block was seen: the coordinators' clock.
    NewBlock { number: u32 },
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
    /// `StorageProvider::ProviderCheckpointSubmitted` or
    /// `StorageProvider::BucketCheckpointed` — new data may be available to
    /// sync for replicas of `bucket_id`.
    BucketCheckpointUpdated { bucket_id: BucketId },
    /// The block follower (re)connected and re-read chain state wholesale.
    /// Coordinators run their bootstrap scan to catch anything missed while
    /// the stream was down. Also the correct reaction to a lagged receiver.
    Resubscribed { at_block: u32 },
}

/// Decode one block's events into the coordinator-relevant [`BlockEvent`]s.
pub fn decode_block_events(events: &subxt::events::Events<PolkadotConfig>) -> Vec<BlockEvent> {
    events
        .iter()
        .filter_map(|event| event.ok())
        .filter(|event| event.pallet_name() == "StorageProvider")
        .filter_map(|event| {
            let name = event.event_name().to_string();
            let fields = event.decode_fields_unchecked_as::<Value>().ok()?;
            block_event_from(&name, &fields)
        })
        .collect()
}

/// Map a decoded `StorageProvider` event to a [`BlockEvent`], if relevant.
///
/// Split out from [`decode_block_events`] so it can be unit-tested against
/// hand-built values without runtime metadata.
fn block_event_from(name: &str, fields: &Value) -> Option<BlockEvent> {
    match name {
        "ChallengeCreated" => {
            let challenge_id = fields.at("challenge_id")?;
            Some(BlockEvent::ChallengeCreated {
                deadline: field_u128(challenge_id, "deadline")? as u32,
                index: field_u128(challenge_id, "index")? as u16,
                bucket_id: field_u128(fields, "bucket_id")? as BucketId,
                provider: field_account(fields, "provider")?,
            })
        }
        "ReplicaAgreementEstablished" => Some(BlockEvent::ReplicaAgreementEstablished {
            bucket_id: field_u128(fields, "bucket_id")? as BucketId,
            provider: field_account(fields, "provider")?,
        }),
        "ProviderCheckpointSubmitted" | "BucketCheckpointed" => {
            Some(BlockEvent::BucketCheckpointUpdated {
                bucket_id: field_u128(fields, "bucket_id")? as BucketId,
            })
        }
        _ => None,
    }
}

fn field_u128(value: &Value, field: &str) -> Option<u128> {
    value.at(field)?.as_u128()
}

fn field_account(value: &Value, field: &str) -> Option<AccountId32> {
    crate::chain_state_coordinator::decode_account(value.at(field)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn challenge_created_fields(provider: [u8; 32]) -> Value {
        Value::named_composite([
            (
                "challenge_id",
                Value::named_composite([
                    ("deadline", Value::u128(1234)),
                    ("index", Value::u128(7)),
                ]),
            ),
            ("bucket_id", Value::u128(42)),
            ("provider", Value::from_bytes(provider)),
            ("challenger", Value::from_bytes([9u8; 32])),
            ("respond_by", Value::u128(1234)),
        ])
    }

    #[test]
    fn decodes_challenge_created() {
        let fields = challenge_created_fields([5u8; 32]);
        let Some(BlockEvent::ChallengeCreated {
            deadline,
            index,
            bucket_id,
            provider,
        }) = block_event_from("ChallengeCreated", &fields)
        else {
            panic!("expected ChallengeCreated");
        };
        assert_eq!(deadline, 1234);
        assert_eq!(index, 7);
        assert_eq!(bucket_id, 42);
        assert_eq!(provider, AccountId32::new([5u8; 32]));
    }

    #[test]
    fn decodes_replica_agreement_established() {
        let fields = Value::named_composite([
            ("bucket_id", Value::u128(3)),
            ("provider", Value::from_bytes([2u8; 32])),
        ]);
        let Some(BlockEvent::ReplicaAgreementEstablished {
            bucket_id,
            provider,
        }) = block_event_from("ReplicaAgreementEstablished", &fields)
        else {
            panic!("expected ReplicaAgreementEstablished");
        };
        assert_eq!(bucket_id, 3);
        assert_eq!(provider, AccountId32::new([2u8; 32]));
    }

    #[test]
    fn decodes_checkpoint_events_to_bucket_update() {
        let fields = Value::named_composite([("bucket_id", Value::u128(11))]);
        for name in ["ProviderCheckpointSubmitted", "BucketCheckpointed"] {
            let Some(BlockEvent::BucketCheckpointUpdated { bucket_id }) =
                block_event_from(name, &fields)
            else {
                panic!("expected BucketCheckpointUpdated for {name}");
            };
            assert_eq!(bucket_id, 11);
        }
    }

    #[test]
    fn irrelevant_or_malformed_events_are_skipped() {
        let fields = Value::named_composite([("bucket_id", Value::u128(1))]);
        assert!(block_event_from("ProviderRegistered", &fields).is_none());
        // ChallengeCreated with missing fields must not panic, just skip.
        assert!(block_event_from("ChallengeCreated", &fields).is_none());
    }
}
