// SPDX-License-Identifier: Apache-2.0

//! Decoded chain events fanned out to the background coordinators.
//!
//! The chain-state coordinator follows finalized blocks on the single chain
//! connection, decodes each block's events once, and broadcasts the
//! coordinator-relevant subset as [`BlockEvent`]s. Coordinators react to
//! events instead of polling storage maps; a slow safety-net scan (and a
//! bootstrap scan on every (re)subscribe) covers anything missed.

use sp_runtime::AccountId32;
use storage_primitives::BucketId;

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
    /// `StorageProvider::BucketCreated` / `MemberSet` / `MemberRemoved` /
    /// `BucketDeleted` / `BucketVisibilityChanged` - the bucket's member set
    /// or visibility changed, so any cached authorization for it is stale.
    /// Only the bucket id is decoded: patching in the member/role the event
    /// carries would build a set that never existed on chain if an earlier
    /// event was missed, so the cache drops the entry and re-resolves.
    BucketMembershipChanged { bucket_id: BucketId },
    /// The block follower (re)connected and re-read chain state wholesale.
    /// Coordinators run their bootstrap scan to catch anything missed while
    /// the stream was down. Also the correct reaction to a lagged receiver.
    Resubscribed { at_block: u32 },
    /// A membership-changing event at `at_block` could not be attributed to a
    /// bucket - its fields failed to decode, so there is no id left to
    /// invalidate. Unlike [`Resubscribed`](Self::Resubscribed), the follower
    /// is still connected and every other coordinator's view of chain state
    /// is unaffected; only the membership cache needs to react, by
    /// distrusting every cached bucket.
    MembershipScopeUnknown { at_block: u32 },
}
