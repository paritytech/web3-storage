// SPDX-License-Identifier: Apache-2.0

//! Typed runtime-event subscription.
//!
//! Streams finalized blocks, decodes each event into the generated
//! [`storage_subxt::api::Event`] runtime enum, and yields the ones that pass
//! an [`EventFilter`].

use crate::event_filter::{BlockEvent, EventFilter};
use crate::{BlockStream, IndexerError};
use futures::{Stream, StreamExt};
use std::pin::Pin;
use std::task::{Context, Poll};
use subxt::{OnlineClient, OnlineClientAtBlock, PolkadotConfig};
use tokio::sync::mpsc;

/// Roughly a few blocks' worth of events; decoded events can be large
/// (BoundedVec payloads), so keep the buffer modest and let `send().await`
/// backpressure a slow consumer.
const CHANNEL_CAPACITY: usize = 256;

/// Most blocks backfilled per connection gap. The gap end is a node-supplied
/// block number, so an uncapped range would let a malicious node force
/// unbounded serial RPC work; one default state-pruning window (256 blocks)
/// also bounds what a non-archive node could resolve anyway.
const MAX_BACKFILL: u64 = 256;

/// A [`Stream`] of filtered, typed runtime events from finalized blocks.
///
/// Internally a background task consumes a [`BlockStream`], fetches each
/// block's events, decodes them into [`storage_subxt::api::Event`], and
/// forwards matches over a channel. Dropping the stream aborts that task
/// (and with it the underlying block stream). Events that fail to decode are
/// logged and skipped.
///
/// # Resilience
///
/// Reconnection is owned entirely by the underlying [`BlockStream`] (see its
/// docs for the backoff behavior). Events from blocks finalized while the
/// connection is down are backfilled best-effort: the stream tracks the last
/// block it processed and, when the subscription resumes past a gap, fetches
/// events for the skipped block numbers in order before processing the new
/// head. At most 256 blocks are backfilled per gap (older blocks are skipped
/// with a warning), and backfill requires the node to still hold state for
/// the missed blocks; blocks whose state has been pruned are skipped with a
/// warning.
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
///     EventStream::connect("ws://localhost:2222", EventFilter::storage_provider()).await?;
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
        Ok(Self::from_blocks(
            BlockStream::connect(ws_url).await?,
            filter,
        ))
    }

    /// Stream events from an existing client, sharing its transport and
    /// already-fetched metadata (see [`BlockStream::from_client`]).
    pub async fn from_client(
        api: OnlineClient<PolkadotConfig>,
        filter: EventFilter,
    ) -> Result<Self, IndexerError> {
        Ok(Self::from_blocks(
            BlockStream::from_client(api).await?,
            filter,
        ))
    }

    /// Stream events from an existing [`BlockStream`].
    pub fn from_blocks(mut blocks: BlockStream, filter: EventFilter) -> Self {
        let (tx, rx) = mpsc::channel::<BlockEvent>(CHANNEL_CAPACITY);

        let task_handle = tokio::spawn(async move {
            let mut last_seen: Option<u64> = None;
            while let Some(block) = blocks.next().await {
                let block_number = block.number();

                // Already delivered (e.g. a reconnect landed on a lagging
                // node); re-forwarding would emit duplicates, and `last_seen`
                // must never move backward.
                if last_seen.is_some_and(|last| block_number <= last) {
                    continue;
                }

                let at = match block.at().await {
                    Ok(at) => at,
                    // `last_seen` is left unchanged, so the next processed
                    // block's gap check backfills this one (no retry before
                    // the first processed block, where `last_seen` is None).
                    Err(e) => {
                        tracing::warn!("Failed to access block {block_number}: {e}");
                        continue;
                    }
                };

                // Backfill blocks skipped over a connection gap (a re-subscribe
                // resumes from the node's current finalized head), capped at
                // MAX_BACKFILL blocks per gap.
                if let Some(last) = last_seen {
                    let next = last.saturating_add(1);
                    let start = next.max(block_number.saturating_sub(MAX_BACKFILL));
                    if start > next {
                        tracing::warn!(
                            "Backfill gap exceeds {MAX_BACKFILL} blocks; \
                             skipping blocks {next}..{start}"
                        );
                    }
                    let api = at.online_client();
                    for missed in start..block_number {
                        // A long catch-up with a narrow filter may never send,
                        // so a departed consumer must be noticed here too.
                        if tx.is_closed() {
                            return;
                        }
                        let missed_at = match api.at_block(missed).await {
                            Ok(at) => at,
                            Err(e) => {
                                tracing::warn!(
                                    "Cannot backfill block {missed} (state pruned?): {e}"
                                );
                                continue;
                            }
                        };
                        if matches!(
                            forward_block_events(&missed_at, &filter, &tx).await,
                            Forwarded::ConsumerGone
                        ) {
                            return;
                        }
                    }
                }

                match forward_block_events(&at, &filter, &tx).await {
                    Forwarded::Processed => last_seen = Some(block_number),
                    // Leave `last_seen` unchanged so the next processed
                    // block's gap check retries this block via backfill.
                    Forwarded::Skipped => {}
                    Forwarded::ConsumerGone => return,
                }
            }
        });

        Self { rx, task_handle }
    }
}

/// Outcome of [`forward_block_events`] for one block.
enum Forwarded {
    /// Events fetched; every filter match was delivered.
    Processed,
    /// Events could not be fetched; the block was not processed.
    Skipped,
    /// The consumer dropped the stream; stop the task.
    ConsumerGone,
}

/// Fetch one resolved block's events and send every one that passes the
/// filter.
async fn forward_block_events(
    at: &OnlineClientAtBlock<PolkadotConfig>,
    filter: &EventFilter,
    tx: &mpsc::Sender<BlockEvent>,
) -> Forwarded {
    let block_hash = at.block_hash();
    let block_number = at.block_number();

    // Response size is bounded by the RPC client's max response size
    // (jsonrpsee default), so a malicious node cannot feed us an arbitrarily
    // large events blob.
    let events = match at.events().fetch().await {
        Ok(events) => events,
        Err(e) => {
            tracing::warn!("Failed to get events for block {block_number}: {e}");
            return Forwarded::Skipped;
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
                // {:?} escapes control characters in these node-supplied
                // names, preventing log injection.
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
            return Forwarded::ConsumerGone;
        }
    }
    Forwarded::Processed
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
