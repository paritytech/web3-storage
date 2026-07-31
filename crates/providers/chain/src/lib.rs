// SPDX-License-Identifier: Apache-2.0

//! Chain connection handling for provider nodes: the single place a subxt
//! client is built, plus the decoded per-block event fan-out consumed by the
//! background coordinators. No HTTP dependencies.

pub mod chain_connection;
pub mod chain_events;
pub mod error;

pub use chain_connection::{connect, current_api, ChainHandle, ChainTransport, ChainWatch};
pub use chain_events::{
    decode_block_events, BlockEvent, BlockEventRx, BlockEventTx, EVENT_CHANNEL_CAPACITY,
};
pub use error::Error;
