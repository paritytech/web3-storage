// SPDX-License-Identifier: Apache-2.0

//! Typed runtime-event subscription.
//!
//! Streams finalized blocks, decodes each event into the generated
//! [`storage_subxt::api::Event`] runtime enum, and yields the ones that pass
//! an [`EventFilter`].

use crate::IndexerError;
use futures::Stream;
use std::collections::HashSet;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use subxt::rpcs::client::{ReconnectingRpcClient, RpcClient};
use subxt::utils::H256;
use subxt::{OnlineClient, PolkadotConfig};
use tokio::sync::mpsc;

/// Roughly a few blocks' worth of events; decoded events can be large
/// (BoundedVec payloads), so keep the buffer modest and let `send().await`
/// backpressure a slow consumer.
const CHANNEL_CAPACITY: usize = 256;

/// First delay once the subscription stops delivering.
const INITIAL_BACKOFF: Duration = Duration::from_secs(1);

/// Ceiling for the exponential backoff.
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// The `pallet-storage-provider` pallet (Layer 0 raw storage).
pub const STORAGE_PROVIDER_PALLET: &str = "StorageProvider";

/// The `pallet-drive-registry` pallet (Layer 1 file system).
pub const DRIVE_REGISTRY_PALLET: &str = "DriveRegistry";

/// The `pallet-s3-registry` pallet (Layer 1 S3).
pub const S3_REGISTRY_PALLET: &str = "S3Registry";

/// The three storage pallets of this chain.
pub const STORAGE_PALLETS: [&str; 3] = [
    STORAGE_PROVIDER_PALLET,
    DRIVE_REGISTRY_PALLET,
    S3_REGISTRY_PALLET,
];

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

/// Filter for selecting which events an [`EventStream`] yields.
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
    /// pallet (Layer 0 raw storage).
    pub fn storage_provider() -> Self {
        Self::pallet(STORAGE_PROVIDER_PALLET)
    }

    /// Match only events from the [`DriveRegistry`](DRIVE_REGISTRY_PALLET)
    /// pallet (Layer 1 file system).
    pub fn drive_registry() -> Self {
        Self::pallet(DRIVE_REGISTRY_PALLET)
    }

    /// Match only events from the [`S3Registry`](S3_REGISTRY_PALLET)
    /// pallet (Layer 1 S3).
    pub fn s3_registry() -> Self {
        Self::pallet(S3_REGISTRY_PALLET)
    }

    /// Match only events from the three storage pallets
    /// (`StorageProvider`, `DriveRegistry`, `S3Registry`).
    pub fn storage_pallets() -> Self {
        Self {
            pallets: Some(STORAGE_PALLETS.into_iter().collect()),
            predicate: None,
        }
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
    fn matches_pallet(&self, pallet_name: &str) -> bool {
        match &self.pallets {
            Some(pallets) => pallets.contains(pallet_name),
            None => true,
        }
    }

    /// Post-decode check: does the decoded event pass the predicate?
    fn matches_event(&self, event: &BlockEvent) -> bool {
        match &self.predicate {
            Some(predicate) => predicate(event),
            None => true,
        }
    }
}

/// A [`Stream`] of filtered, typed runtime events from finalized blocks.
///
/// Internally a background task owns the block subscription, fetches each
/// block's events, decodes them into [`storage_subxt::api::Event`], and
/// forwards matches over a channel. Dropping the stream aborts that task.
/// Events that fail to decode are logged and skipped.
///
/// # Resilience
///
/// The stream connects over a reconnecting WebSocket transport, and if the
/// block subscription itself stops delivering it is re-established with capped
/// exponential backoff (1s doubling to 30s, reset once blocks flow again).
/// Events from blocks finalized while the connection is down are NOT
/// backfilled — after a reconnect the stream resumes from the node's current
/// finalized head.
///
/// # Example
///
/// ```no_run
/// use futures::StreamExt;
/// use storage_indexers::{EventFilter, EventStream};
/// use storage_subxt::api;
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let mut stream =
///     EventStream::connect("ws://localhost:2222", EventFilter::storage_pallets()).await?;
/// while let Some(ev) = stream.next().await {
///     if let api::Event::StorageProvider(event) = ev.event {
///         println!("block {}: {event:?}", ev.block_number);
///     }
/// }
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct EventStream {
    rx: mpsc::Receiver<BlockEvent>,
    task_handle: tokio::task::JoinHandle<()>,
}

impl EventStream {
    /// Connect to a node by WebSocket URL and start streaming events.
    pub async fn connect(ws_url: &str, filter: EventFilter) -> Result<Self, IndexerError> {
        let rpc = ReconnectingRpcClient::builder().build(ws_url).await?;
        let api = OnlineClient::<PolkadotConfig>::from_rpc_client(RpcClient::new(rpc)).await?;
        let mut block_sub = api.stream_blocks().await?;

        let (tx, rx) = mpsc::channel::<BlockEvent>(CHANNEL_CAPACITY);

        let task_handle = tokio::spawn(async move {
            // Throttles every non-delivery path below; reset only when a block
            // actually arrives, so a flapping connection keeps escalating
            // instead of hot-looping on instantly-succeeding re-subscribes.
            let mut backoff = INITIAL_BACKOFF;
            loop {
                let block = match block_sub.next().await {
                    Some(Ok(block)) => {
                        backoff = INITIAL_BACKOFF;
                        block
                    }
                    // Transient item error (e.g. a reconnect notice from the
                    // transport); the subscription itself is still alive, but
                    // throttle so persistent errors cannot spin.
                    Some(Err(e)) => {
                        tracing::warn!("Block subscription error: {e}");
                        tokio::select! {
                            // Consumer gone: stop.
                            _ = tx.closed() => return,
                            _ = tokio::time::sleep(backoff) => {}
                        }
                        backoff = (backoff * 2).min(MAX_BACKOFF);
                        continue;
                    }
                    // Subscription ended (typically connection loss) →
                    // re-subscribe. The transport reconnects on its own; this
                    // re-establishes the subscription on top of it.
                    None => {
                        tracing::warn!("Block subscription ended; re-subscribing in {backoff:?}");
                        block_sub = loop {
                            tokio::select! {
                                // Consumer gone: stop retrying.
                                _ = tx.closed() => return,
                                _ = tokio::time::sleep(backoff) => {}
                            }
                            backoff = (backoff * 2).min(MAX_BACKOFF);
                            match api.stream_blocks().await {
                                Ok(sub) => break sub,
                                Err(e) => tracing::warn!(
                                    "Re-subscribe failed: {e}; retrying in {backoff:?}"
                                ),
                            }
                        };
                        continue;
                    }
                };

                let block_hash = block.hash();
                let block_number = block.number();

                let block_ref = match block.at().await {
                    Ok(block_ref) => block_ref,
                    Err(e) => {
                        tracing::warn!("Failed to access block {block_number}: {e}");
                        continue;
                    }
                };
                // Response size is bounded by the RPC client's max response
                // size (jsonrpsee default), so a malicious node cannot feed us
                // an arbitrarily large events blob.
                let events = match block_ref.events().fetch().await {
                    Ok(events) => events,
                    Err(e) => {
                        tracing::warn!("Failed to get events for block {block_number}: {e}");
                        continue;
                    }
                };

                for event in events.iter() {
                    let event = match event {
                        Ok(event) => event,
                        Err(e) => {
                            tracing::warn!("Failed to read event: {e}");
                            continue;
                        }
                    };
                    if !filter.matches_pallet(event.pallet_name()) {
                        continue;
                    }
                    let decoded = match event.decode_as::<storage_subxt::api::Event>() {
                        Ok(decoded) => decoded,
                        Err(e) => {
                            // {:?} escapes control characters in these
                            // node-supplied names, preventing log injection.
                            tracing::warn!(
                                "Failed to decode {:?}.{:?}: {e}",
                                event.pallet_name(),
                                event.event_name()
                            );
                            continue;
                        }
                    };
                    let block_event = BlockEvent {
                        block_hash,
                        block_number,
                        event: decoded,
                    };
                    if !filter.matches_event(&block_event) {
                        continue;
                    }
                    // Receiver dropped → consumer is gone, stop streaming.
                    if tx.send(block_event).await.is_err() {
                        return;
                    }
                }
            }
        });

        Ok(Self { rx, task_handle })
    }
}

impl Drop for EventStream {
    fn drop(&mut self) {
        // Stop the background subscription task promptly even if it is parked
        // on the next finalized block.
        self.task_handle.abort();
    }
}

impl Stream for EventStream {
    type Item = BlockEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.rx.poll_recv(cx)
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
        let filter = EventFilter::pallet("StorageProvider");
        assert!(filter.matches_pallet("StorageProvider"));
        assert!(!filter.matches_pallet("DriveRegistry"));
        assert!(!filter.matches_pallet("Balances"));

        let filter = filter.with_pallet("DriveRegistry");
        assert!(filter.matches_pallet("DriveRegistry"));
        assert!(!filter.matches_pallet("Balances"));
    }

    #[test]
    fn per_pallet_constructors_select_only_their_pallet() {
        let cases = [
            (EventFilter::storage_provider(), "StorageProvider"),
            (EventFilter::drive_registry(), "DriveRegistry"),
            (EventFilter::s3_registry(), "S3Registry"),
        ];
        for (filter, own_pallet) in cases {
            assert!(filter.matches_pallet(own_pallet));
            for other in STORAGE_PALLETS.into_iter().filter(|p| *p != own_pallet) {
                assert!(!filter.matches_pallet(other));
            }
            assert!(!filter.matches_pallet("Balances"));
        }
    }

    #[test]
    fn storage_pallets_selects_the_three() {
        let filter = EventFilter::storage_pallets();
        for pallet in STORAGE_PALLETS {
            assert!(filter.matches_pallet(pallet));
        }
        assert!(!filter.matches_pallet("System"));
        assert!(!filter.matches_pallet("Balances"));
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
