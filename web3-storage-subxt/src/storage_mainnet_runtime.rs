// Placeholder for the mainnet runtime bindings.
//
// The `mainnet` feature is wired up in `lib.rs` (it re-exports this module as
// `storage_runtime`), but the metadata has not been generated yet. Until then,
// building with `--features mainnet` fails with the message below instead of a
// confusing missing-symbol error.
//
// To implement: generate the subxt bindings for the mainnet runtime into this
// file (the same way `storage_paseo_runtime.rs` is generated), then delete this
// `compile_error!`.
compile_error!(
    "the `mainnet` feature is not yet implemented: generate the mainnet runtime \
     metadata into src/storage_mainnet_runtime.rs, then remove this compile_error!"
);