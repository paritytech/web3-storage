# Web3-Storage-Subxt

Static [`subxt`](https://github.com/paritytech/subxt) bindings generated from the
web3-storage runtime metadata (`metadata/storage_paseo_runtime.scale`).

### Regenerating the bindings

The metadata download and code generation are driven by the `subxt-codegen`
recipe in the repository [`justfile`](../../justfile) — see the recipe for the
exact `subxt` CLI invocation and derive flags. With a node running
(`just start-paseo-chain` in another terminal), run:

```bash
just subxt-codegen
```

This refreshes `metadata/storage_paseo_runtime.scale` and regenerates
`src/storage_paseo_runtime.rs`.
