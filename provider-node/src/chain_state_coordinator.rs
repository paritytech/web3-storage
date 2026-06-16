//! Chain-state coordinator: keeps the provider node's view of the runtime in
//! sync via a finalized-block subscription.
//!
//! Currently exposes [`ChainState`], the slice of [`crate::ProviderState`] that
//! is kept live with the chain. The subscription loop and refresh task are added
//! on top of this type.

use std::sync::atomic::AtomicU32;
use parking_lot::RwLock;
use storage_client::discovery::ProviderInfo;

/// Live chain state kept in sync with the runtime by the chain-state coordinator.
///
/// Held behind `Arc` inside [`crate::ProviderState`] so the coordinator can hold
/// its own handle without a back-reference to the whole node state.
#[derive(Default)]
pub struct ChainState {
    /// Latest finalized block height. `0` means not yet known — the coordinator
    /// writes the real value once it first connects.
    pub current_block: AtomicU32,
    /// Provider's on-chain registration info. `None` until first fetch; updated
    /// whenever a settings or multiaddr-change event lands.
    pub provider_info: RwLock<Option<ProviderInfo>>,
    /// Mirror StorageProvider::Config::RequestTimeout
    pub request_timeout: u64,
}
