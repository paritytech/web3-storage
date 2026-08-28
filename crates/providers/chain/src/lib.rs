// SPDX-License-Identifier: Apache-2.0

//! Chain connection handling for provider nodes: the single place a subxt
//! client is built, plus the decoded per-block event fan-out consumed by the
//! background coordinators. No HTTP dependencies.
//!
//! The event vocabulary ([`BlockEvent`] and friends) is always available.
//! Decoding raw on-chain events into it (`decode` feature) and building the
//! subxt connection itself (`connection` feature) are each optional, so a
//! consumer that only reacts to already-decoded events - the challenge
//! responder, say - does not have to compile subxt.

#[cfg(feature = "connection")]
pub mod chain_connection;
#[cfg(feature = "connection")]
pub mod error;
#[cfg(feature = "decode")]
pub mod event_decoding;
pub mod events;

#[cfg(feature = "connection")]
pub use chain_connection::{
    connect, current_api, ChainHandle, ChainTransport, ChainWatch, SpecSource,
};
#[cfg(feature = "connection")]
pub use error::Error;
#[cfg(feature = "decode")]
pub use event_decoding::decode_block_events;
pub use events::{BlockEvent, BlockEventRx, BlockEventTx, EVENT_CHANNEL_CAPACITY};
