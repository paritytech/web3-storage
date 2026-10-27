// SPDX-License-Identifier: Apache-2.0

//! Shared provider-related types.
//!
//! Main types:
//! * [`ProviderInfo`], [`ProviderSettings`], [`ProviderStats`] : the node's view of its
//!   on-chain provider registration.
//! * [`ProviderKeypair`], [`KeyScheme`], [`SigningRefused`] : the signing key, its scheme,
//!   and the reasons the node cannot sign.
//! * [`ChainClientError`] : the error the chain-client traits of the provider crates report.
//! * [`Balance`], [`BlockNumber`] - the runtime's numeric types.
//!
//! Inputs: none. Pure data types and signing helpers, with no chain connection or I/O.
//!
//! Outputs: values the other provider crates and the node pass between each other.

mod chain;
mod keys;
mod provider_info;
mod runtime_type;

pub use chain::ChainClientError;
pub use keys::{KeyScheme, ProviderKeypair, SigningRefused};
pub use provider_info::{ProviderInfo, ProviderSettings, ProviderStats};
pub use runtime_type::{Balance, BlockNumber};
