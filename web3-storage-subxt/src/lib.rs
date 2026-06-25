#![deny(stable_features, non_shorthand_field_patterns, renamed_and_removed_lints, unsafe_code)]

pub use codec;
pub use scale_info;
#[cfg(any(feature = "std"))]
pub use subxt;
#[cfg(any(feature = "std"))]
pub use subxt::ext::subxt_core;
#[cfg(not(any(feature = "std")))]
pub use subxt_core;
pub use subxt_signer;

#[cfg(feature = "mainnet")]
#[rustfmt::skip]
pub mod storage_mainnet_runtime;
#[cfg(feature = "mainnet")]
pub use storage_mainnet_runtime as storage_runtime;

#[cfg(not(feature = "mainnet"))]
#[rustfmt::skip]
pub mod storage_paseo_runtime;
#[cfg(not(feature = "mainnet"))]
pub use storage_paseo_runtime as storage_runtime;
