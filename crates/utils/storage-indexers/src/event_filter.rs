// SPDX-License-Identifier: Apache-2.0

//! The decoded event type and the filter that selects which events an
//! [`EventStream`](crate::EventStream) yields.

use std::collections::HashSet;
use std::sync::Arc;
use subxt::utils::H256;

/// The `pallet-storage-provider` pallet (buckets, agreements, challenges).
pub const STORAGE_PROVIDER_PALLET: &str = "StorageProvider";

/// A decoded runtime event together with the block it was emitted in.
#[derive(Clone, Debug)]
pub struct BlockEvent {
    /// Hash of the finalized block the event was emitted in.
    pub block_hash: H256,
    /// Number of the finalized block the event was emitted in.
    pub block_number: u64,
    /// The decoded runtime event.
    pub event: storage_subxt::api::Event,
}

/// Predicate applied to decoded events.
type Predicate = Arc<dyn Fn(&BlockEvent) -> bool + Send + Sync>;

/// Filter for selecting which events an [`EventStream`](crate::EventStream) yields.
///
/// Filtering happens in two stages:
///
/// 1. A pallet gate on the raw event's pallet name, applied BEFORE decoding —
///    events from non-selected pallets are never decoded.
/// 2. An optional predicate over the decoded [`BlockEvent`] for anything
///    finer-grained (a specific bucket, provider, variant, ...).
#[derive(Clone, Default)]
pub struct EventFilter {
    /// Pallet names to decode; `None` = all pallets.
    pallets: Option<HashSet<&'static str>>,
    predicate: Option<Predicate>,
}

impl std::fmt::Debug for EventFilter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventFilter")
            .field("pallets", &self.pallets)
            .field("predicate", &self.predicate.is_some())
            .finish()
    }
}

impl EventFilter {
    /// Match every event from every pallet.
    pub fn all() -> Self {
        Self::default()
    }

    /// Match only events from the given pallet.
    pub fn pallet(name: &'static str) -> Self {
        Self::default().with_pallet(name)
    }

    /// Match only events from the [`StorageProvider`](STORAGE_PROVIDER_PALLET)
    /// pallet (buckets, agreements, challenges).
    pub fn storage_provider() -> Self {
        Self::pallet(STORAGE_PROVIDER_PALLET)
    }

    /// Add a pallet to the selection (narrows an [`all`](Self::all) filter).
    pub fn with_pallet(mut self, name: &'static str) -> Self {
        self.pallets.get_or_insert_default().insert(name);
        self
    }

    /// Set a predicate over the decoded event.
    pub fn with_predicate(
        mut self,
        predicate: impl Fn(&BlockEvent) -> bool + Send + Sync + 'static,
    ) -> Self {
        self.predicate = Some(Arc::new(predicate));
        self
    }

    /// Pre-decode gate: should events from this pallet be decoded at all?
    pub(crate) fn matches_pallet(&self, pallet_name: &str) -> bool {
        match &self.pallets {
            Some(pallets) => pallets.contains(pallet_name),
            None => true,
        }
    }

    /// Post-decode check: does the decoded event pass the predicate?
    pub(crate) fn matches_event(&self, event: &BlockEvent) -> bool {
        match &self.predicate {
            Some(predicate) => predicate(event),
            None => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use storage_subxt::api;
    use storage_subxt::api::runtime_types::pallet_storage_provider::pallet::Event as SpEvent;

    fn bucket_deleted(bucket_id: u64) -> BlockEvent {
        BlockEvent {
            block_hash: H256::zero(),
            block_number: 1,
            event: api::Event::StorageProvider(SpEvent::BucketDeleted { bucket_id }),
        }
    }

    #[test]
    fn all_matches_every_pallet() {
        let filter = EventFilter::all();
        assert!(filter.matches_pallet("StorageProvider"));
        assert!(filter.matches_pallet("Balances"));
        assert!(filter.matches_event(&bucket_deleted(1)));
    }

    #[test]
    fn pallet_gate_selects_only_named_pallets() {
        let filter = EventFilter::storage_provider();
        assert!(filter.matches_pallet(STORAGE_PROVIDER_PALLET));
        assert!(!filter.matches_pallet("Balances"));

        let filter = filter.with_pallet("Balances");
        assert!(filter.matches_pallet("Balances"));
        assert!(!filter.matches_pallet("System"));
    }

    #[test]
    fn predicate_composes_with_pallet_gate() {
        let filter = EventFilter::pallet("StorageProvider").with_predicate(|ev| {
            matches!(
                ev.event,
                api::Event::StorageProvider(SpEvent::BucketDeleted { bucket_id: 42 })
            )
        });
        assert!(filter.matches_pallet("StorageProvider"));
        assert!(!filter.matches_pallet("Balances"));
        assert!(filter.matches_event(&bucket_deleted(42)));
        assert!(!filter.matches_event(&bucket_deleted(7)));
    }
}
