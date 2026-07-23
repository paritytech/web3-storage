// SPDX-License-Identifier: Apache-2.0

//! Finalized-block subscription.

use crate::IndexerError;
use futures::Stream;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;
use subxt::client::Block;
use subxt::rpcs::client::{ReconnectingRpcClient, RpcClient};
use subxt::{OnlineClient, PolkadotConfig};
use tokio::sync::mpsc;

/// A few blocks of buffer; with ~6s block time a deeper buffer would only
/// delay backpressure onto the subscription.
const CHANNEL_CAPACITY: usize = 32;

/// First delay once the subscription stops delivering.
const INITIAL_BACKOFF: Duration = Duration::from_secs(1);

/// Ceiling for the exponential backoff.
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// A finalized block as delivered by the subscription.
type FinalizedBlock = Block<PolkadotConfig>;

/// A [`Stream`] of finalized blocks.
///
/// Internally a background task owns the subxt subscription and forwards each
/// block over a channel. Dropping the stream aborts that task.
///
/// # Resilience
///
/// The stream connects over a reconnecting WebSocket transport, and if the
/// block subscription itself stops delivering it is re-established with capped
/// exponential backoff (1s doubling to 30s, reset once blocks flow again).
/// Blocks finalized while the connection is down are NOT backfilled — after a
/// reconnect the stream resumes from the node's current finalized head.
///
/// # Example
///
/// ```no_run
/// use futures::StreamExt;
/// use storage_indexers::BlockStream;
///
/// async fn example() -> Result<(), Box<dyn std::error::Error>> {
///   let mut stream = BlockStream::connect("ws://localhost:2222").await?;
///   while let Some(block) = stream.next().await {
///     println!("finalized block {}", block.number());
///   }
///   Ok(())
/// }
/// ```
#[derive(Debug)]
pub struct BlockStream {
    rx: mpsc::Receiver<FinalizedBlock>,
    task_handle: tokio::task::JoinHandle<()>,
}

impl BlockStream {
    /// Connect to a node by WebSocket URL and start streaming finalized blocks.
    pub async fn connect(ws_url: &str) -> Result<Self, IndexerError> {
        let rpc = ReconnectingRpcClient::builder().build(ws_url).await?;
        let api = OnlineClient::<PolkadotConfig>::from_rpc_client(RpcClient::new(rpc)).await?;
        let mut block_sub = api.stream_blocks().await?;

        let (tx, rx) = mpsc::channel::<FinalizedBlock>(CHANNEL_CAPACITY);

        let task_handle = tokio::spawn(async move {
            // Throttles every non-delivery path below; reset only when a block
            // actually arrives, so a flapping connection keeps escalating
            // instead of hot-looping on instantly-succeeding re-subscribes.
            let mut backoff = INITIAL_BACKOFF;
            loop {
                match block_sub.next().await {
                    Some(Ok(block)) => {
                        backoff = INITIAL_BACKOFF;
                        // Receiver dropped → consumer is gone, stop streaming.
                        if tx.send(block).await.is_err() {
                            return;
                        }
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
                    }
                }
            }
        });

        Ok(Self { rx, task_handle })
    }
}

impl Drop for BlockStream {
    fn drop(&mut self) {
        // Stop the background subscription task promptly even if it is parked
        // on the next finalized block.
        self.task_handle.abort();
    }
}

impl Stream for BlockStream {
    type Item = FinalizedBlock;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.rx.poll_recv(cx)
    }
}
