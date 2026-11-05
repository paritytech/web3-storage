// SPDX-License-Identifier: Apache-2.0

//! Node-side mirrors of the runtime's primitive aliases, declared in
//! `runtimes/web3-storage-*/src/lib.rs`. Add more as mirrored types need them.
//! Plain aliases, not newtypes: they document intent, they don't enforce it.
//!
//! TODO: maybe candidate to go to the `storage-primitives`

/// Balance of an account. Mirrors the runtime's `Balance`.
pub type Balance = u128;

/// Block number type. Mirrors the runtime's `BlockNumber`.
pub type BlockNumber = u32;
