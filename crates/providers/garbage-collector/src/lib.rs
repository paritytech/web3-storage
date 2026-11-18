// SPDX-License-Identifier: Apache-2.0

//! Garbage collector: erases pruned and torn-down bucket data once on-chain
//! liability for it has ended.
//!
//! The crate reaches the chain, local storage and chain events only through
//! the traits in this crate; the provider node supplies the implementations.

mod error;
mod traits;

pub use error::{BoxError, GcError};
pub use traits::{
    CanonicalBucketState, EraseOutcome, GcChainClient, GcEventSource, GcStorage, GcTrigger,
    PrunedRange,
};
