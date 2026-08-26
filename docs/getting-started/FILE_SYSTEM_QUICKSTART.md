# File System Quick Start Guide

Quick commands to test and run the Layer 1 File System Interface.

## Prerequisites

```bash
cargo install just            # or: brew install just
just setup                    # one-time: download binaries + build everything
```

## Run It

The file-system layer talks to a running parachain + provider node, so you need
three terminals. None of the test recipes start the chain or provider for you.

```bash
# Terminal 1 — relay chain + parachain (parachain WS on ws://127.0.0.1:2222)
just start-chain

# Terminal 2 — provider node (HTTP on http://127.0.0.1:3333, signing as //Alice)
just start-provider

# Terminal 3 — file-system integration example (chain + provider must be up)
just fs-demo-ci
```

`fs-demo-ci` runs `clients/file-system/examples/ci_integration_test.rs`
against the running infrastructure: it creates a drive via the `DriveRegistry`
pallet, exercises directory and file operations through the provider's
`/fs/{bucket}/...` HTTP endpoints, and asserts the round-trip. It assumes the
chain and provider are already up.

This is the same flow exercised by CI in `.github/workflows/integration-tests.yml`,
which spins up zombienet + providers before invoking the recipe.

## Tests

```bash
just fs-test-all              # primitives + drive registry pallet + client (unit tests)
cargo test --workspace        # everything
```

## Health Checks

```bash
bash scripts/check-chain.sh   # relay (9900) + parachain (2222) + current block
just health                   # provider node /health
just stats                    # provider storage stats
```

## Network endpoints (defaults)

| Service              | Endpoint                                                  |
|----------------------|-----------------------------------------------------------|
| Relay chain RPC      | `ws://127.0.0.1:9900`                                     |
| Parachain RPC        | `ws://127.0.0.1:2222`                                     |
| Provider HTTP        | `http://127.0.0.1:3333`                                   |
| Polkadot.js (relay)  | `https://polkadot.js.org/apps/?rpc=ws://127.0.0.1:9900`   |
| Polkadot.js (chain)  | `https://polkadot.js.org/apps/?rpc=ws://127.0.0.1:2222`   |

Override via the `RELAY_PORT`, `CHAIN_PORT`, `PROVIDER_PORT` justfile variables
if you need different ports.

## Troubleshooting

### Cannot connect

```bash
bash scripts/check-chain.sh   # parachain producing blocks?
just health                   # provider responding?
```

### Ports already in use

```bash
lsof -ti:2222 | xargs kill    # parachain
lsof -ti:9900 | xargs kill    # relay chain
lsof -ti:3333 | xargs kill    # provider
```

### Clean start

```bash
pkill -f polkadot
pkill -f storage-provider-node
pkill -f zombienet
just build
# then re-run the three terminals above
```

## Further reading

- [User Guide](../filesystems/USER_GUIDE.md) — complete user workflows
- [Architecture](../filesystems/ARCHITECTURE.md) — encoding, security, chain integration
- [API Reference](../filesystems/API_REFERENCE.md) — complete API docs
- [Client README](../../clients/file-system/README.md) — SDK docs
- [Architecture](../filesystems/ARCHITECTURE.md) — encoding, security, chain integration
