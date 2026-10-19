// SPDX-License-Identifier: Apache-2.0

//! Decoded chain events fanned out to the background coordinators.
//!
//! The chain-state coordinator follows finalized blocks on the single chain
//! connection, decodes each block's events once, and broadcasts the
//! coordinator-relevant subset as [`BlockEvent`]s. Coordinators react to
//! events instead of polling storage maps; a slow safety-net scan (and a
//! bootstrap scan on every (re)subscribe) covers anything missed.

mod events;

pub use events::{BlockEvent, BlockEventRx, BlockEventTx, EVENT_CHANNEL_CAPACITY};
