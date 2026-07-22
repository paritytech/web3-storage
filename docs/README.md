# Documentation

```
docs/
├── getting-started/   — quickstart
├── reference/         — pallet API + payment math
├── design/            — canonical system design (review-gated)
├── drafts/            — work-in-progress protocol notes (need triage)
└── filesystems/       — Layer 1 file system interface
```

> `design/` is the source of truth and requires design review before changes.
> `drafts/` holds evolving notes that have not been ratified — treat as WIP.

## Getting started

- **[Layer 1 Quick Start](./getting-started/LAYER1_QUICKSTART.md)** — three-terminal setup (chain → provider → demo) plus SDK examples for the file-system and S3 interfaces. The canonical entry point.
- **[`FILE_SYSTEM_QUICKSTART.md`](../FILE_SYSTEM_QUICKSTART.md)** (repo root) — short version, file-system layer only.
- **[`CLAUDE.md`](../CLAUDE.md)** (repo root) — build/test/run commands and contributor guidelines.

## Reference

- **[Extrinsics Reference](./reference/EXTRINSICS_REFERENCE.md)** — every pallet extrinsic with parameters, errors, and example workflows.
- **[Payment Calculator](./reference/PAYMENT_CALCULATOR.md)** — `payment = price_per_byte × max_bytes × duration`, with worked examples.

## Design

The canonical system design. Changes require review (see [`.github/CODEOWNERS`](../.github/CODEOWNERS)).

- **[Scalable Web3 Storage](./design/scalable-web3-storage.md)** — architecture, economic model, comparisons with Filecoin/IPFS/Arweave, rebuttals to common review concerns.
- **[Implementation Details](./design/scalable-web3-storage-implementation.md)** — pallet extrinsics, provider HTTP API, MMR layout, challenge mechanism, replica sync.

## Drafts

Work-in-progress notes, not yet ratified. **These need triage** — each should be reviewed and either promoted into `design/`, folded into an existing doc, or dropped.

- **[Layer 1 Design / Implementation](./drafts/L1_design_implementation.md)** — file-system & S3 provider interfaces on top of Layer 0 (split out of the Layer 0 implementation doc); triage tracked in [#51](https://github.com/paritytech/web3-storage/issues/51).
- **[Smart Contracts](./drafts/smart-contracts.md)** — `pallet_revive` integration, custom precompile ABI, address mapping, payment flow. Candidate for promotion to `reference/` (it's API-reference material, not a draft design).
- **[Execution Flows](./drafts/EXECUTION_FLOWS.md)** — sequence-by-sequence walkthroughs for the main flows.
- **[Marketplace](./drafts/marketplace.md)** — provider capacity, discovery, and matching.
- **[Checkpoint Protocol](./drafts/CHECKPOINT_PROTOCOL.md)** — multi-provider checkpoint coordination.
- **[Provider-Initiated Checkpoints](./drafts/provider-initiated-checkpoints.md)** — extension where providers proactively commit state.
- **[Client-Side Encryption](./drafts/CLIENT_SIDE_ENCRYPTION.md)** — wire format, cipher choice.
- **[S3 Metadata Index](./drafts/S3_METADATA_INDEX.md)** — how prefix/delimiter queries are served.
- **[Challenge Economics — Extensions](./drafts/challenge-economics-extensions.md)** — speculative "Capped Split for the general public"; also records that the design's two-tier challenger split isn't implemented yet.

## Layer 1 — file system interface

- **[Layer 1 README](./filesystems/README.md)** — sub-index for the file-system layer.
- **[Architecture](./filesystems/ARCHITECTURE.md)** — encoding, security, blockchain integration.
- **[User Guide](./filesystems/USER_GUIDE.md)** — drive creation, file/dir operations, configuration.
- **[Admin Guide](./filesystems/ADMIN_GUIDE.md)** — operations, monitoring, dispute resolution.
- **[API Reference](./filesystems/API_REFERENCE.md)** — extrinsics, SDK methods, primitives, events, errors.

## Clients

- **[Storage Client SDK](../client/README.md)** — Layer-0 Rust client.
- **[S3 Interface](../storage-interfaces/s3/README.md)** — Layer-1 S3-compatible Rust client.
- **TypeScript SDK** — `@web3-storage/sdk` at `packages/sdk` (`./fs`, `./s3`, `./revive` subpaths).

## External

- [Polkadot SDK](https://paritytech.github.io/polkadot-sdk/) — FRAME, Cumulus, networking.
- [Substrate Docs](https://docs.substrate.io/).
- [Polkadot.js Apps](https://polkadot.js.org/apps/).

## License

See the repository root [README](../README.md#license).
