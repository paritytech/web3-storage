# Example Solidity contracts

`StorageMarketplace.sol` is a tiny dApp that buys storage on behalf of its users via the web3-storage precompile at `0x0000000000000000000000000000000009010000`. It's the demo target for `just sc-demo` (the smart-contract integration test introduced by issue #83).

## Build

```bash
./build.sh
```

Outputs land in `./build/` (gitignored): `StorageMarketplace.bin` (PolkaVM bytecode) and `StorageMarketplace.abi` (ABI JSON).

## Toolchain

Two binaries must be on `PATH`:

| Tool     | Version target          | Where to get it                                                 |
|----------|-------------------------|------------------------------------------------------------------|
| `solc`   | 0.8.x                   | https://github.com/ethereum/solidity/releases                    |
| `resolc` | matches `pallet-revive` | https://github.com/paritytech/revive/releases                    |

The exact pinned versions live in `.github/env` and are installed identically in CI (`SOLC_VERSION`, `RESOLC_VERSION`).

`resolc` is Parity's Solidity-to-PolkaVM frontend — it invokes `solc` internally to parse Solidity, then compiles the IR to PolkaVM bytecode.

## Files

| File                    | Purpose                                                  |
|-------------------------|----------------------------------------------------------|
| `IWeb3Storage.sol`      | Vendored ABI of the precompile (kept in sync with `precompiles/storage-provider-precompile/src/IWeb3Storage.sol`). |
| `StorageMarketplace.sol`| Marketplace contract: `buyStorage` / `endMyAgreement`.    |
| `build.sh`              | One-shot compile script.                                 |
