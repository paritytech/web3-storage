// SPDX-License-Identifier: Apache-2.0

//! Shared provider identity and registration types.

mod keys;
mod registration;

pub use keys::{KeyScheme, ProviderKeypair};
pub use registration::ProviderInfo;
