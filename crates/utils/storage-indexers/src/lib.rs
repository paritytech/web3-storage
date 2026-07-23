// SPDX-License-Identifier: Apache-2.0

//! Indexing primitives for the Web3 Storage chain.
//!
//! Two building blocks, both implementing [`futures::Stream`]:
//!
//! - [`BlockStream`] — finalized blocks.
//! - [`EventStream`] — typed runtime events ([`storage_subxt::api::Event`])
//!   with block context, filtered by pallet and an optional predicate.
//!
//! Events are decoded through the generated [`storage_subxt`] bindings, so
//! consumers match on generated types instead of re-declaring event enums.

mod block_stream;
mod error;
mod event_stream;

pub use block_stream::BlockStream;
pub use error::IndexerError;
pub use event_stream::{
    BlockEvent, EventFilter, EventStream, DRIVE_REGISTRY_PALLET, S3_REGISTRY_PALLET,
    STORAGE_PALLETS, STORAGE_PROVIDER_PALLET,
};

pub use storage_subxt;
