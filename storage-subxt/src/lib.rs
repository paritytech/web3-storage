#![deny(
    stable_features,
    non_shorthand_field_patterns,
    renamed_and_removed_lints,
    unsafe_code
)]

pub use codec;
pub use scale_info;
#[cfg(feature = "std")]
pub use subxt;
#[cfg(feature = "std")]
pub use subxt::ext::subxt_core;
#[cfg(not(feature = "std"))]
pub use subxt_core;
pub use subxt_signer;

#[rustfmt::skip]
pub mod storage_paseo_runtime;

#[cfg(feature = "mainnet")]
#[rustfmt::skip]
pub mod storage_mainnet_runtime;
#[cfg(feature = "mainnet")]
pub use storage_mainnet_runtime as storage_runtime;

#[cfg(not(feature = "mainnet"))]
pub use storage_paseo_runtime as storage_runtime;
