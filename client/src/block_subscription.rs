//! Finalized-block subscription.
//!
//! Provides [`BlockSubscriberStream`], a [`Stream`] of finalized blocks: a
//! background task owns the subxt subscription and forwards each block over a
//! channel, so a consumer can `.await` per block (e.g. fetch the block's events
//! and issue a chain query in response).

use crate::ClientError;
use futures::Stream;
use std::pin::Pin;
use std::task::{Context, Poll};
use subxt::blocks::Block;
use subxt::{OnlineClient, PolkadotConfig};
use tokio::sync::mpsc;

/// A finalized block as delivered by the subscription.
type FinalizedBlock = Block<PolkadotConfig, OnlineClient<PolkadotConfig>>;

/// A [`Stream`] of finalized blocks.
///
/// Internally a background task owns the subxt subscription and forwards each
/// block over a channel. Dropping the stream aborts that task.
///
/// # Example
///
/// ```no_run
/// use storage_client::block_subscription::BlockSubscriberStream;
/// use futures::StreamExt;
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let mut stream = BlockSubscriberStream::connect("ws://localhost:2222").await?;
/// while let Some(block) = stream.next().await {
///     println!("finalized block {}", block.number());
/// }
/// # Ok(())
/// # }
/// ```
pub struct BlockSubscriberStream {
    rx: mpsc::Receiver<FinalizedBlock>,
    task_handle: tokio::task::JoinHandle<()>,
}

impl BlockSubscriberStream {
    /// Connect to a node by WebSocket URL and start streaming finalized blocks.
    pub async fn connect(ws_url: &str) -> Result<Self, ClientError> {
        let api = OnlineClient::<PolkadotConfig>::from_url(ws_url)
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to connect: {e}")))?;
        Self::from_client(api).await
    }

    /// Start streaming on top of an existing subxt client.
    pub async fn from_client(api: OnlineClient<PolkadotConfig>) -> Result<Self, ClientError> {
        let mut block_sub = api
            .blocks()
            .subscribe_finalized()
            .await
            .map_err(|e| ClientError::Chain(format!("Failed to subscribe to blocks: {e}")))?;

        let (tx, rx) = mpsc::channel::<FinalizedBlock>(32);

        let task_handle = tokio::spawn(async move {
            loop {
                match block_sub.next().await {
                    // Receiver dropped → consumer is gone, stop streaming.
                    Some(Ok(block)) => {
                        if tx.send(block).await.is_err() {
                            break;
                        }
                    }
                    Some(Err(e)) => tracing::error!("Block subscription error: {e}"),
                    None => break,
                }
            }
        });

        Ok(Self { rx, task_handle })
    }
}

impl Drop for BlockSubscriberStream {
    fn drop(&mut self) {
        // Stop the background subscription task promptly even if it is parked
        // on the next finalized block.
        self.task_handle.abort();
    }
}

impl Stream for BlockSubscriberStream {
    type Item = FinalizedBlock;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.rx.poll_recv(cx)
    }
}
