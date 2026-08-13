// SPDX-License-Identifier: Apache-2.0

//! Static subxt bindings for the storage parachain runtime.
//!
//! The bindings are generated from the paseo runtime, and the other runtimes in
//! this workspace share the pallets they cover, so call sites read and write
//! through `unvalidated()` addresses: exact-hash validation would pin the binary
//! to a single runtime build for no safety gain, since a real shape mismatch
//! still fails at decode.

#![deny(
    stable_features,
    non_shorthand_field_patterns,
    renamed_and_removed_lints,
    unsafe_code
)]

pub use codec;
pub use scale_info;
pub use subxt;
pub use subxt_signer;

#[rustfmt::skip]
pub mod storage_paseo_runtime;
pub use storage_paseo_runtime::*;

mod primitive_conversions;
