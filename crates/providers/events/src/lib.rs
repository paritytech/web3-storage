// SPDX-License-Identifier: Apache-2.0

//! The decoded per-block event fan-out consumed by the background
//! coordinators. No HTTP dependencies.

pub mod events;

pub use events::{BlockEvent, BlockEventRx, BlockEventTx, EVENT_CHANNEL_CAPACITY};
