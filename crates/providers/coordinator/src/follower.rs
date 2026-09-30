// SPDX-License-Identifier: Apache-2.0

//! Following the chain's finalized blocks: building a connection, subscribing
//! to it, and what each block yields to the coordinator.
//!
//! The concrete transport (subxt or otherwise) is never named here, so the
//! reconnect loop can be driven by a mock in tests without a live chain.

use crate::{ChainStateChainClient, Error, ProviderLifecycleEvent};
use async_trait::async_trait;
use provider_events::BlockEvent;
use std::sync::Arc;

/// Builds chain connections for the coordinator's reconnect loop.
#[async_trait]
pub trait ChainFollower: Send + Sync {
    /// Build a fresh connection. Not yet subscribed or published to other
    /// chain consumers — see [`ChainSession::subscribe`].
    async fn connect(&self) -> Result<Box<dyn ChainSession>, Error>;
}

/// One established chain connection, before it is subscribed and published.
#[async_trait]
pub trait ChainSession: Send + Sync {
    /// Subscribe to finalized blocks and publish this connection to the
    /// node's other chain consumers. Returns the block stream and the reads
    /// client bound to this same connection, so both are guaranteed to agree
    /// on which chain they are talking to.
    async fn subscribe(
        self: Box<Self>,
    ) -> Result<(Box<dyn FinalizedBlocks>, Arc<dyn ChainStateChainClient>), Error>;
}

/// The finalized-block stream the coordinator drives.
#[async_trait]
pub trait FinalizedBlocks: Send {
    /// Next finalized block, or `None` when the stream ends.
    async fn next(&mut self) -> Option<BlockUpdate>;
}

/// What the coordinator learns from one finalized block.
pub enum BlockUpdate {
    /// The block was read successfully.
    Block(FinalizedBlock),
    /// The block's handle or events could not be read at all, so any
    /// membership events it carried are lost rather than merely dropped one
    /// at a time — the same "no bucket id left to invalidate" situation an
    /// undecodable event escalates to.
    Unreadable { number: u32 },
}

/// Everything the coordinator needs out of one successfully-read finalized
/// block.
pub struct FinalizedBlock {
    pub number: u32,
    /// The pallet's anchor block at this block, or `None` if the read
    /// failed — the caller keeps the previous value rather than resetting it.
    pub anchor_block: Option<u32>,
    /// Coordinator-relevant events, already decoded, ready to fan out.
    pub events: Vec<BlockEvent>,
    /// `StorageProvider` provider-lifecycle events, already decoded.
    pub lifecycle: Vec<ProviderLifecycleEvent>,
}
