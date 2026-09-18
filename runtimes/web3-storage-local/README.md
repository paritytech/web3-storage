# Parachain Runtime

Substrate parachain runtime that composes the storage provider pallet, drive
registry pallet, `pallet_revive` (smart contracts), and standard FRAME pallets
into a complete blockchain runtime.

## Build

```bash
cargo build -p storage-parachain-runtime --release

# With benchmarks
cargo build -p storage-parachain-runtime --release --features runtime-benchmarks
```

## License

Licensed under [GPL-3.0-only](../LICENSE-GPL3).
