# zombienet-sdk-tests

Spawns a local relay chain + parachain network using the [zombienet SDK](https://github.com/nicovank/zombienet-sdk) and runs integration tests against it.

## Prerequisites

Download Polkadot SDK binaries and build the project:

```bash
just download-binaries
just build
```

This places `polkadot`, `polkadot-omni-node`, and `chain-spec-builder` into `.bin/`.

## Usage

### Local dev network

```bash
just start-chain          # relay chain + parachain
just start-all            # same + storage provider
```

### Integration tests

```bash
just demo                 # Layer 0: full storage flow
just fs-demo-ci           # Layer 1: file system
just s3-demo-ci           # Layer 1: S3-compatible
```

Or directly:

```bash
cargo test --release -p zombienet-sdk-tests --features zombie-tests layer0 -- --nocapture
```

## Environment variables

All settings have sensible defaults. Override via env vars when needed:

| Variable | Default | Description |
|---|---|---|
| `POLKADOT_BINARY_PATH` | `.bin/polkadot` | Polkadot relay chain binary |
| `POLKADOT_OMNI_NODE_PATH` | `.bin/polkadot-omni-node` | Parachain omni-node binary |
| `CHAIN_SPEC_COMMAND` | `./scripts/build-chain-spec.sh` | Chain spec generation script |
| `PROVIDER_BINARY_PATH` | `./target/release/storage-provider-node` | Storage provider binary |
| `RELAY_RPC_PORT` | `9900` | Relay chain RPC port |
| `CHAIN_RPC_PORT` | `2222` | Parachain RPC port |
| `PROVIDER_PORT` | `3333` | Storage provider HTTP port |

## Adding a new test

1. Create a test file, e.g. `tests/layer1/my_feature.rs`
2. Add a `mod.rs` if the parent module doesn't have one, or add `pub mod my_feature;` to the existing one
3. Wire it into `tests/tests.rs` if it's a new top-level module
4. Use `TestEnvironment::spawn().await?` to get a running network + provider (see existing tests)
5. Add an entry to `.github/zombie-tests.yml` so CI runs it in its own job:

```yaml
  - name: "Layer 1: My Feature"
    id: layer1-my-feature        # artifact-safe name (no colons/spaces)
    filter: "layer1::my_feature"  # cargo test filter
    timeout: 30
```

Each entry in `zombie-tests.yml` runs as a separate CI matrix job. The `filter` is passed to `cargo test` and the `id` is used for artifact names (must not contain `:` or spaces).

## Crate structure

- `src/` — shared library (network config, provider lifecycle) + `smoke` binary
- `tests/` — integration tests gated behind the `zombie-tests` feature flag
  - `common/` — shared test helpers (`TestEnvironment`, provider registration)
  - `layer0/` — Layer 0 storage tests
  - `layer1/` — Layer 1 interface tests (filesystem, S3)
