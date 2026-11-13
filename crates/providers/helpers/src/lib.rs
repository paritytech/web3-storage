// SPDX-License-Identifier: Apache-2.0

//! Provider management operations for operator tooling: generate the
//! provider's keyfile, register a provider, update its settings, and
//! announce its deregistration.
//!
//! [`ProviderHelper`] contains the operation logic: it validates the input
//! and the provider's current on-chain state before it submits a
//! transaction, so an operator gets a clear error instead of a rejected
//! transaction and a wasted fee.

mod chain;
mod helper;
mod keyfile;
mod types;

pub use chain::{ChainError, ProviderChain};
pub use helper::{HelperError, ProviderHelper, SettingsUpdateOutcome};
pub use keyfile::{
    generate_mnemonic, write_keyfile, Keyfile, KeyfileError, DEFAULT_MNEMONIC_WORDS,
    MNEMONIC_WORD_COUNTS,
};
pub use types::{
    ProviderRecord, ProviderRegistration, ProviderSettings, ProviderSettingsUpdate, SettingsError,
    TxReceipt,
};
