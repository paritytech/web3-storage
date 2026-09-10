# Documentation

```
docs/
├── getting-started/   — quickstart
├── design/            — canonical system design (review-gated)
└── drafts/            — unratified / WIP notes (need triage)
```

> - **`design/`** — the source of truth; changes require design-owner review (see [`.github/CODEOWNERS`](../.github/CODEOWNERS)).
> - **`drafts/`** — unratified / WIP notes; treat as provisional. Each needs triage: promote to `design/`, fold into an existing doc, or drop.

## API reference

There is no hand-written API reference. The rustdoc on the pallets is the reference, and because that text ships in the runtime metadata, every client surface shows the same words:

- **Rust** — `cargo doc --workspace --no-deps --open`, or hover over the `storage-subxt` bindings in your IDE.
- **TypeScript** — hover over the PAPI descriptors generated from `packages/papi`; the JSDoc is the pallet rustdoc.
- **Browser** — [polkadot.js Apps](https://polkadot.js.org/apps/?rpc=ws://127.0.0.1:2222) → Developer → Extrinsics shows the doc for each call, error, and event.

A pallet item without a doc comment fails CI: the pallets set `#![warn(missing_docs)]` and clippy runs with `-D warnings`.

## Getting started

- **[Quick Start](./getting-started/FILE_SYSTEM_QUICKSTART.md)** — three-terminal setup (chain → provider → demo).
- **[`CLAUDE.md`](../CLAUDE.md)** (repo root) — agent/contributor rules and the source-of-truth map.

## Design

The canonical system design. Changes require review (see [`.github/CODEOWNERS`](../.github/CODEOWNERS)).

- **[Scalable Web3 Storage](./design/scalable-web3-storage.md)** — architecture, economic model, comparisons with Filecoin/IPFS/Arweave, rebuttals to common review concerns.
- **[Implementation Details](./design/scalable-web3-storage-implementation.md)** — pallet extrinsics, provider HTTP API, MMR layout, challenge mechanism, replica sync.

## Drafts

Unratified / WIP notes. **These need triage** ([#308](https://github.com/paritytech/web3-storage/issues/308)) — each should be reviewed and either promoted into `design/` (design-of-record), folded into an existing doc, or dropped.

- **[Layer 1 Design / Implementation](./drafts/L1_design_implementation.md)** — file-system & S3 provider interfaces on top of Layer 0 (split out of the Layer 0 implementation doc); triage tracked in [#51](https://github.com/paritytech/web3-storage/issues/51).
- **[Smart Contracts](./drafts/smart-contracts.md)** — `pallet_revive` integration, custom precompile ABI, address mapping, payment flow.
- **[Marketplace](./drafts/marketplace.md)** — provider capacity, discovery, and matching.
- **[Checkpoint Protocol](./drafts/CHECKPOINT_PROTOCOL.md)** — multi-provider checkpoint coordination.
- **[Provider-Initiated Checkpoints](./drafts/provider-initiated-checkpoints.md)** — extension where providers proactively commit state; removed in #306, archived (design + implementation) for potential re-evaluation.
- **[Client-Side Encryption](./drafts/CLIENT_SIDE_ENCRYPTION.md)** — wire format, cipher choice.
- **[S3 Metadata Index](./drafts/S3_METADATA_INDEX.md)** — how prefix/delimiter queries are served.
- **[Challenge Economics — Extensions](./drafts/challenge-economics-extensions.md)** — speculative "Capped Split for the general public"; also records that the design's two-tier challenger split isn't implemented yet.

## Clients

- **[Storage Client SDK](../clients/storage/README.md)** — Layer-0 Rust client.
- **[File System Client](../clients/file-system/README.md)** — Layer-1 drives, directories, and files.
- **[S3 Interface](../clients/s3/README.md)** — Layer-1 S3-compatible Rust client.
- **TypeScript SDK** — `@web3-storage/sdk` at `packages/sdk` (`./fs`, `./s3`, `./revive` subpaths).

## External

- [Polkadot SDK](https://paritytech.github.io/polkadot-sdk/) — FRAME, Cumulus, networking.
- [Substrate Docs](https://docs.substrate.io/).
- [Polkadot.js Apps](https://polkadot.js.org/apps/).

## License

See the repository root [README](../README.md#license).
