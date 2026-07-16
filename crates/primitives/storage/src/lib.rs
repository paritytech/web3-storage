// SPDX-License-Identifier: Apache-2.0

//! Shared primitives for Scalable Web3 Storage
//!
//! This crate contains types and structures shared between the on-chain pallet
//! and off-chain provider node.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod agreement;
pub mod agreement_term;
pub mod bucket;
pub mod challenge;
pub mod checkpoint;
pub mod commitment;
pub mod mmr;
pub mod provider;
pub mod provider_replay_state;

pub use agreement::*;
pub use agreement_term::*;
pub use bucket::*;
pub use challenge::*;
pub use checkpoint::*;
pub use commitment::*;
pub use mmr::*;
pub use provider::*;
pub use provider_replay_state::*;

/// Bucket ID is a stable, unique identifier (not an index into a collection).
/// Using u64 ensures IDs never get reused even if buckets are deleted.
pub type BucketId = u64;

/// Default chunk size: 256 KiB
pub const DEFAULT_CHUNK_SIZE: u32 = 256 * 1024;

/// Prime numbers used for historical root bucketing.
/// These provide logarithmic time coverage for replica sync validation.
pub const HISTORICAL_ROOT_PRIMES: [u32; 6] = [3, 7, 11, 23, 47, 113];
