# Documentation

Project structure:

```
docs/
├── getting-started/   — quickstart
├── reference/         — pallet API + payment math
├── design/            — architecture, economics, protocols
└── filesystems/       — Layer 1 file system interface
```

## Getting started

- **[Layer 1 Quick Start](./getting-started/LAYER1_QUICKSTART.md)** — three-terminal setup (chain → provider → demo) plus the SDK examples for the file-system and S3 interfaces. The canonical entry point.
- **[`FILE_SYSTEM_QUICKSTART.md`](../FILE_SYSTEM_QUICKSTART.md)** (repo root) — short version focused on the file-system layer only.
- **[`CLAUDE.md`](../CLAUDE.md)** (repo root) — build/test/run commands and code-review guidelines for contributors and AI agents.

## Reference

- **[Extrinsics Reference](./reference/EXTRINSICS_REFERENCE.md)** — every pallet extrinsic with parameters, errors, and example workflows.
- **[Payment Calculator](./reference/PAYMENT_CALCULATOR.md)** — `payment = price_per_byte × max_bytes × duration`, with worked examples and the common `PaymentExceedsMax` failure mode.

## Design

- **[Scalable Web3 Storage](./design/scalable-web3-storage.md)** — architecture, economic model, comparisons with Filecoin/IPFS/Arweave, rebuttals to common review concerns.
- **[Implementation Details](./design/scalable-web3-storage-implementation.md)** — pallet extrinsics, provider HTTP API, MMR layout, challenge mechanism, replica sync.
- **[Smart Contracts](./design/smart-contracts.md)** — `pallet_revive` integration, custom precompile ABI, address mapping, payment flow.
- **[Execution Flows](./design/EXECUTION_FLOWS.md)** — sequence-by-sequence walkthroughs for the main flows.
- **[Marketplace](./design/marketplace.md)** — provider capacity, discovery, and matching.
- **[Checkpoint Protocol](./design/CHECKPOINT_PROTOCOL.md)** — multi-provider checkpoint coordination.
- **[Provider-Initiated Checkpoints](./design/provider-initiated-checkpoints.md)** — extension where providers proactively commit state.
- **[Client-Side Encryption](./design/CLIENT_SIDE_ENCRYPTION.md)** — wire format, cipher choice.
- **[S3 Metadata Index](./design/S3_METADATA_INDEX.md)** — how prefix/delimiter queries are served.

## Layer 1 — file system interface

- **[Layer 1 README](./filesystems/README.md)** — sub-index for the file-system layer.
- **[Architecture](./filesystems/ARCHITECTURE.md)** — encoding, security, blockchain integration.
- **[User Guide](./filesystems/USER_GUIDE.md)** — drive creation, file/dir operations, configuration.
- **[Admin Guide](./filesystems/ADMIN_GUIDE.md)** — operations, monitoring, dispute resolution.
- **[API Reference](./filesystems/API_REFERENCE.md)** — extrinsics, SDK methods, primitives, events, errors.

## Other clients

- **[Storage Client SDK](../client/README.md)** — Layer-0 Rust client.
- **[S3 Interface](../storage-interfaces/s3/README.md)** — Layer-1 S3-compatible Rust client.
- **TypeScript SDKs** under `user-interfaces/sdk/typescript/{file-system,s3}/`.

## Scripts

- `scripts/build-chain-spec.sh` — build runtime + emit chain spec (used by `just generate-chain-spec`).
- `scripts/check-chain.sh` — relay + parachain health probe.
- `scripts/quick-test.sh` — curl-based smoke test of the Layer-0 provider HTTP API.

## External

- [Polkadot SDK](https://paritytech.github.io/polkadot-sdk/) — FRAME, Cumulus, networking.
- [Substrate Docs](https://docs.substrate.io/).
- [Polkadot.js Apps](https://polkadot.js.org/apps/).

## License

See the repository root [README](../README.md#license) for license details.
