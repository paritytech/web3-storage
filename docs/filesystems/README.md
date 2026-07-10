# File System Interface (Layer 1)

A high-level abstraction over Layer 0's raw blob storage: drives, directories, files. Users don't interact with buckets, agreements, or providers directly — those are managed by the file-system layer.

## Documents

| Document | Audience |
|----------|----------|
| [ARCHITECTURE.md](./ARCHITECTURE.md) | Encoding, security, encryption, chain integration |
| [USER_GUIDE.md](./USER_GUIDE.md) | Drive/file/directory operations, configuration |
| [ADMIN_GUIDE.md](./ADMIN_GUIDE.md) | System management, provider operations, dispute handling |
| [API_REFERENCE.md](./API_REFERENCE.md) | Extrinsics, SDK methods, primitives, events, errors |

## Getting started

Three terminals (full setup in [LAYER1_QUICKSTART.md](../getting-started/LAYER1_QUICKSTART.md)):

```bash
just start-chain        # Terminal 1
just start-provider     # Terminal 2
just fs-demo-ci         # Terminal 3 — integration example, requires the above
```

Or for the basic SDK example:

```bash
cargo run -p file-system-client --example basic_usage
```

## Components

- **Drive Registry pallet** (`crates/pallets/drive-registry/`) — on-chain drive metadata, owner mapping, root CID slot.
- **File-system primitives** (`storage-interfaces/file-system/primitives/`) — shared types: `DriveInfo`, `DirectoryNode`, `FileManifest`, `CommitStrategy`.
- **File-system client SDK** (`storage-interfaces/file-system/client/`) — Rust client. Real chain queries via subxt; file operations via the provider's `/fs/{bucket}/...` HTTP endpoints.
- **TypeScript SDK** — `FileSystemClient` from `@web3-storage/sdk/fs` (`packages/layer1`), built on the layer-0 chain binding.

## Related

- [Layer 0 Design](../design/scalable-web3-storage.md) — the underlying storage system.
- [Layer 0 Implementation](../design/scalable-web3-storage-implementation.md) — pallet, provider, MMR, challenges.
- [Checkpoint Protocol](../design/CHECKPOINT_PROTOCOL.md) — multi-provider checkpoint coordination used by Layer 1's commit strategies.

## License

See the repository root [README](../../README.md#license) for license details.
