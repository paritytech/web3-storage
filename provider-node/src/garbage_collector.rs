// SPDX-License-Identifier: GPL-3.0-only

//! Adapters connecting the garbage collector crate to the node's storage
//! backend and block events.

use provider_events::{BlockEvent, BlockEventRx};
use provider_garbage_collector::{
    EraseOutcome, GcError, GcEventSource, GcStorage, GcTrigger, PrunedRange,
};
use provider_storage::{GarbageCollectorConfiguration, StorageBackend};
use std::sync::Arc;
use storage_primitives::BucketId;
use tokio::sync::broadcast::error::RecvError;

/// [`GcStorage`] over the node's storage backend.
pub struct BackendGcStorage(pub Arc<dyn StorageBackend>);

impl GcStorage for BackendGcStorage {
    fn bucket_ids(&self) -> Vec<BucketId> {
        self.0.bucket_ids()
    }

    fn bucket_start_seq(&self, bucket_id: BucketId) -> Result<Option<u64>, GcError> {
        self.0
            .get_bucket(bucket_id)
            .map(|bucket| bucket.map(|b| b.start_seq))
            .map_err(GcError::storage)
    }

    fn last_scanned_block(&self) -> Result<u64, GcError> {
        self.0
            .garbage_collector_configuration()
            .map(|c| c.last_scanned_block)
            .map_err(GcError::storage)
    }

    fn set_last_scanned_block(&self, block_number: u64) -> Result<(), GcError> {
        self.0
            .set_garbage_collector_configuration(&GarbageCollectorConfiguration {
                last_scanned_block: block_number,
            })
            .map_err(GcError::storage)
    }

    fn is_condemned(&self, bucket_id: BucketId) -> Result<bool, GcError> {
        self.0.is_condemned(bucket_id).map_err(GcError::storage)
    }

    fn condemn_bucket(&self, bucket_id: BucketId) -> Result<(), GcError> {
        self.0.condemn_bucket(bucket_id).map_err(GcError::storage)
    }

    fn init_bucket_or_set_quota(&self, bucket_id: BucketId, max_bytes: u64) -> Result<(), GcError> {
        self.0
            .init_bucket_or_set_quota(bucket_id, max_bytes)
            .map_err(GcError::storage)
    }

    fn delete_before(&self, bucket_id: BucketId, new_start_seq: u64) -> Result<(), GcError> {
        self.0
            .delete_before(bucket_id, new_start_seq)
            .map(|_| ())
            .map_err(GcError::storage)
    }

    fn pruned_ranges(&self, bucket_id: BucketId) -> Result<Vec<PrunedRange>, GcError> {
        self.0
            .pruned_ranges(bucket_id)
            .map(|ranges| {
                ranges
                    .into_iter()
                    .map(|r| PrunedRange {
                        first_seq: r.first_seq,
                        end_seq: r.end_seq,
                        new_start_seq: r.new_start_seq,
                        has_receipt: r.has_receipt,
                    })
                    .collect()
            })
            .map_err(GcError::storage)
    }

    fn erase_pruned_range(
        &self,
        bucket_id: BucketId,
        first_seq: u64,
    ) -> Result<EraseOutcome, GcError> {
        self.0
            .erase_pruned_range(bucket_id, first_seq)
            .map(|o| EraseOutcome {
                nodes_deleted: o.nodes_deleted,
                bytes_freed: o.bytes_freed,
            })
            .map_err(GcError::storage)
    }
}

/// [`GcEventSource`] over the block-event fan-out.
pub struct BlockEventGcSource(pub BlockEventRx);

#[async_trait::async_trait]
impl GcEventSource for BlockEventGcSource {
    async fn next(&mut self) -> Option<GcTrigger> {
        loop {
            match self.0.recv().await {
                Ok(BlockEvent::Resubscribed { .. }) | Err(RecvError::Lagged(_)) => {
                    return Some(GcTrigger::Rescan);
                }
                Ok(BlockEvent::BucketCheckpointed { bucket_id, .. })
                | Ok(BlockEvent::BucketDeleted { bucket_id })
                | Ok(BlockEvent::AgreementChanged { bucket_id, .. }) => {
                    return Some(GcTrigger::Bucket(bucket_id));
                }
                Ok(_) => {}
                Err(RecvError::Closed) => return None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use provider_events::BlockEvent;
    use sp_runtime::AccountId32;

    fn provider() -> AccountId32 {
        AccountId32::new([1; 32])
    }

    #[tokio::test]
    async fn maps_events_to_triggers_and_skips_unrelated_ones() {
        let (tx, rx) = tokio::sync::broadcast::channel(16);
        let mut source = BlockEventGcSource(rx);

        tx.send(BlockEvent::ChallengeCreated {
            deadline: 1,
            index: 0,
            bucket_id: 9,
            provider: provider(),
        })
        .unwrap();
        tx.send(BlockEvent::BucketCheckpointed {
            bucket_id: 1,
            start_seq: 4,
        })
        .unwrap();
        tx.send(BlockEvent::BucketMembershipChanged { bucket_id: 9 })
            .unwrap();
        tx.send(BlockEvent::BucketDeleted { bucket_id: 2 }).unwrap();
        tx.send(BlockEvent::AgreementChanged {
            bucket_id: 3,
            provider: provider(),
        })
        .unwrap();
        tx.send(BlockEvent::Resubscribed { at_block: 7 }).unwrap();

        assert_eq!(source.next().await, Some(GcTrigger::Bucket(1)));
        assert_eq!(source.next().await, Some(GcTrigger::Bucket(2)));
        assert_eq!(source.next().await, Some(GcTrigger::Bucket(3)));
        assert_eq!(source.next().await, Some(GcTrigger::Rescan));
    }

    #[tokio::test]
    async fn lagged_receiver_requests_a_rescan() {
        let (tx, rx) = tokio::sync::broadcast::channel(2);
        let mut source = BlockEventGcSource(rx);

        for bucket_id in 0..4 {
            tx.send(BlockEvent::BucketDeleted { bucket_id }).unwrap();
        }

        assert_eq!(source.next().await, Some(GcTrigger::Rescan));
    }

    #[tokio::test]
    async fn closed_channel_ends_the_source() {
        let (tx, rx) = tokio::sync::broadcast::channel(2);
        let mut source = BlockEventGcSource(rx);
        drop(tx);

        assert_eq!(source.next().await, None);
    }
}
