// SPDX-License-Identifier: Apache-2.0

//! Error type for the indexer streams.

/// Errors produced while connecting to or subscribing against a chain node.
#[derive(Debug, thiserror::Error)]
pub enum IndexerError {
    /// Failed to establish the underlying WebSocket connection.
    #[error("failed to connect RPC transport: {0}")]
    Transport(#[from] subxt::rpcs::client::reconnecting_rpc_client::RpcError),

    /// Failed to initialize the client on top of an established connection.
    #[error("failed to connect to node: {0}")]
    Connect(#[from] subxt::error::OnlineClientError),

    /// Failed to open the finalized-block subscription.
    #[error("failed to subscribe to blocks: {0}")]
    Subscribe(#[from] subxt::error::BlocksError),
}
