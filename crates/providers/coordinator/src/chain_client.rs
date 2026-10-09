// SPDX-License-Identifier: Apache-2.0

//! The chain reads the coordinator performs to keep its state in sync.

use crate::Error;
use async_trait::async_trait;
use provider_types::ProviderInfo;
use sp_runtime::AccountId32;

/// The chain reads the coordinator needs to keep [`ChainState`](crate::ChainState) in sync.
///
/// Abstracted behind a trait — exactly like the other coordinators'
/// `*ChainClient` traits — so the [`sync_constants`](crate::sync_constants) /
/// [`refresh_provider_state`](crate::refresh_provider_state) logic can be
/// driven by a mock in tests without a live chain.
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
