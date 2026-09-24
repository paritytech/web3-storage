// SPDX-License-Identifier: Apache-2.0

//! The decoded per-block event fan-out consumed by the background
//! coordinators. No HTTP dependencies.
//!
//! The event vocabulary ([`BlockEvent`] and friends) is always available.
//! Decoding raw on-chain events into it (`decode` feature) is optional, so a
//! consumer that only reacts to already-decoded events - the challenge
//! responder, say - does not have to compile subxt.

#[cfg(feature = "decode")]
pub mod event_decoding;
pub mod events;

#[cfg(feature = "decode")]
pub use event_decoding::decode_block_events;
pub use events::{BlockEvent, BlockEventRx, BlockEventTx, EVENT_CHANNEL_CAPACITY};
