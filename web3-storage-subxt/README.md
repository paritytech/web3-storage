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
    | rustfmt --edition=2021 --emit=stdout > ./src/storage_paseo_runtime.rs
```
