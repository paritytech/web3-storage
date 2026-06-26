# Web3-Storage-Subxt

### Downloading metadata from a Substrate node

Use the [`subxt-cli`](https://crates.io/crates/subxt-cli) tool to download the metadata for your target runtime from a node.

1. Install:

```bash
cargo install subxt-cli@0.44.3 --force --locked
```

2. To Save the metadata of runtime:
    Run the release build of the web3-storage runtime:
    ```rust
    just start-paseo-chain
    ```

    Then on another terminal run:
    ```bash
    subxt metadata -f bytes --url ws://localhost:2222 > ./metadata/storage-paseo-runtime.scale
    ```

3. Generating the subxt code from the metadata:

```bash
subxt codegen --file ./metadata/storage_paseo_runtime.scale \
    --crate "::subxt_core" \
    --derive Clone \
    --derive Eq \
    --derive PartialEq \
    --derive-for-type "pallet_storage_provider::pallet::ProviderInfo=serde::Serialize" \
    --derive-for-type "pallet_storage_provider::pallet::ProviderInfo=serde::Deserialize" \
    --derive-for-type "pallet_storage_provider::pallet::ProviderSettings=serde::Serialize" \
    --derive-for-type "pallet_storage_provider::pallet::ProviderSettings=serde::Deserialize" \
    --derive-for-type "pallet_storage_provider::pallet::ProviderStats=serde::Serialize" \
    --derive-for-type "pallet_storage_provider::pallet::ProviderStats=serde::Deserialize" \
    --derive-for-type "bounded_collections::bounded_vec::BoundedVec=serde::Serialize" \
    --derive-for-type "bounded_collections::bounded_vec::BoundedVec=serde::Deserialize" \
    --derive-for-type "sp_runtime::MultiSignature=codec::Encode" \
    --derive-for-type "sp_runtime::MultiSignature=codec::Decode" \
    | rustfmt --edition=2021 --emit=stdout > ./src/storage_paseo_runtime.rs
```
