// SPDX-License-Identifier: Apache-2.0

//! The chain interface the coordinator drives. It names no transport, so the
//! node supplies the subxt implementation and tests supply mocks.

use crate::{Error, ProviderLifecycleEvent};
use async_trait::async_trait;
use provider_chain::BlockEvent;
use provider_types::ProviderInfo;
use sp_runtime::AccountId32;

/// Opens chain connections for the coordinator's reconnect loop.
#[async_trait]
pub trait ChainFollower: Send + Sync {
    /// Connect, subscribe to finalized blocks, and publish the new connection
    /// to the node's other chain consumers. Implementations must not publish
    /// before the subscription succeeds.
    async fn connect(&self) -> Result<ChainConnection, Error>;
}

/// A connected chain: the finalized-block stream and a reads client on the
/// same connection.
pub struct ChainConnection {
    /// Finalized blocks of this connection.
    pub blocks: Box<dyn FinalizedBlocks>,
    /// Chain reads on this connection.
    pub client: Box<dyn ChainStateChainClient>,
}

/// The chain reads the coordinator needs to keep [`ChainState`](crate::ChainState) in sync.
#[async_trait]
pub trait ChainStateChainClient: Send + Sync {
    /// Full on-chain `ProviderInfo`, or `None` if the provider is not registered.
    async fn get_provider_info(&self, who: &AccountId32) -> Result<Option<ProviderInfo>, Error>;

    /// Provider's replay-window head sequence (`hsn`), or `None` if no replay
    /// state exists yet (the provider has never signed any terms).
    async fn fetch_replay_hsn(&self, who: &AccountId32) -> Result<Option<u64>, Error>;

    /// `StorageProvider::RequestTimeout` runtime constant, or `None` if absent
    /// from the node's metadata.
    async fn fetch_request_timeout(&self) -> Result<Option<u32>, Error>;
}

/// A stream of finalized blocks.
#[async_trait]
pub trait FinalizedBlocks: Send {
    /// Next finalized block, or `None` when the stream ends or fails.
    async fn next(&mut self) -> Option<BlockUpdate>;
}

/// One finalized block, as the coordinator receives it.
pub enum BlockUpdate {
    /// The block was read.
    Block(FinalizedBlock),
    /// The block's handle or events could not be read.
    Unreadable { number: u32 },
}

/// A finalized block with its events already decoded.
pub struct FinalizedBlock {
    pub number: u32,
    /// The pallet's anchor block at this block, or `None` if the read failed.
    pub anchor_block: Option<u32>,
    /// Events to forward to the other coordinators.
    pub events: Vec<BlockEvent>,
    /// `StorageProvider` provider-lifecycle events.
    pub lifecycle: Vec<ProviderLifecycleEvent>,
}
