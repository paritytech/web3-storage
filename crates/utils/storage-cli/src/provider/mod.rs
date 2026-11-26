// SPDX-License-Identifier: Apache-2.0

//! Provider management operations for operator tooling: generate a
//! keyfile, register a provider, update its settings, and announce its
//! deregistration.
//!
//! [`ProviderManager`] contains the operation logic: it validates the input
//! and the provider's current on-chain state before it submits a
//! transaction, so an operator gets a clear error instead of a rejected
//! transaction and a wasted fee.

mod keyfile;
mod manager;

pub use keyfile::{generate_mnemonic, write_keyfile, DEFAULT_MNEMONIC_WORDS, MNEMONIC_WORD_COUNTS};
pub use manager::ProviderManager;

/// The generic Substrate SS58 prefix, 42: how an account is displayed when
/// the chain's own `System::SS58Prefix` cannot be read.
pub const GENERIC_SS58_PREFIX: u16 = 42;
