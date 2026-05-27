# Smart Contracts

Web3-storage exposes its client-side surface to Solidity / PolkaVM contracts via `pallet_revive` and two custom precompiles. A dApp can buy storage on behalf of its users, end agreements, or stitch the storage pallets into larger on-chain workflows — without ever signing a substrate-native extrinsic.

This document covers the **on-chain shape** of that integration. For the example dApp, see [`examples/contracts/`](../../examples/contracts/README.md); for the test driver, see [`examples/papi/sc-flow.js`](../../examples/papi/sc-flow.js).

## Architecture

```
JS test  →  pallet_revive.call (or eth_transact)
                ↓
           PolkaVM contract (StorageMarketplace.sol)
                ↓ CALL 0x…0901
           StorageProviderPrecompile  (Rust, native)
                ↓ ensure_signed(contract_account)
           pallet_storage_provider::Pallet::*  →  provider HTTP node
```

The contract's caller is computed by `AccountId32Mapper` (substrate-native, identity-of-derivation): `keccak_256(account_bytes)[12..]` forward, `OriginalAccount` reverse-lookup with `[H160 || 0xEE*12]` fallback. The precompile builds `RawOrigin::Signed(env.caller())` and dispatches as if the contract had signed the extrinsic itself.

## Precompile address map

`AddressMatcher::Fixed(u16)` places the u16 at **bytes 16-17** of the H160 (big-endian) with a `0x0000` suffix at bytes 18-19. Bytes 18-19 are reserved for built-in precompiles only. So `Fixed(0x0901)` yields address `0x00000000000000000000000000000000_0901_0000`:

| Address                                       | Matcher           | Crate                                                                                                 | Pallet                       |
| --------------------------------------------- | ----------------- | ----------------------------------------------------------------------------------------------------- | ---------------------------- |
| `0x0000000000000000000000000000000009010000`  | `Fixed(0x0901)`   | [`pallet-storage-provider-precompile`](../../precompiles/storage-provider-precompile)                 | `pallet_storage_provider`    |
| `0x0000000000000000000000000000000009020000`  | `Fixed(0x0902)`   | [`pallet-drive-registry-precompile`](../../precompiles/drive-registry-precompile)                     | `pallet_drive_registry`      |
| `0x0000000000000000000000000000000009030000`  | `Fixed(0x0903)`   | [`pallet-s3-registry-precompile`](../../precompiles/s3-registry-precompile)                           | `pallet_s3_registry`         |

Both use `HAS_CONTRACT_INFO = false` (no storage deposits or contract metadata; pure stateless dispatch).

## ABI surface

### `IWeb3Storage` (storage-provider, `0x…09010000`)

| Selector                                                                                                            | Underlying extrinsic                                |
| ------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------- |
| `createBucket(uint32 minProviders) → uint64`                                                                        | `create_bucket`                                     |
| `createBucketWithStorage(uint64 maxBytes, uint32 duration, uint128 maxPricePerByte) → uint64`                       | `create_bucket_with_storage` (auto-matches provider) |
| `freezeBucket(uint64 bucketId)`                                                                                     | `freeze_bucket`                                     |
| `setMember(uint64 bucketId, bytes32 member, uint8 role)`                                                            | `set_member`                                        |
| `removeMember(uint64 bucketId, bytes32 member)`                                                                     | `remove_member`                                     |
| `requestPrimaryAgreement(uint64 bucketId, bytes32 provider, uint64 maxBytes, uint32 duration, uint128 maxPayment)`  | `request_primary_agreement`                         |
| `topUpAgreement(uint64 bucketId, bytes32 provider, uint64 additionalBytes, uint128 maxPayment)`                     | `top_up_agreement`                                  |
| `extendAgreement(uint64 bucketId, bytes32 provider, uint32 additionalDuration, uint128 maxPayment)`                 | `extend_agreement`                                  |
| `endAgreementPay(uint64 bucketId, bytes32 provider)`                                                                | `end_agreement` (action: `Pay`)                     |
| `endAgreementBurn(uint64 bucketId, bytes32 provider, uint8 burnPercent)`                                            | `end_agreement` (action: `Burn { burn_percent }`)   |
| `challengeCheckpoint(uint64 bucketId, bytes32 provider, uint64 leafIndex, uint64 chunkIndex)`                       | `challenge_checkpoint`                              |

### `IDriveRegistry` (drive-registry, `0x…09020000`)

| Selector                                                                                                              | Underlying extrinsic |
| --------------------------------------------------------------------------------------------------------------------- | -------------------- |
| `createDrive(string name, uint64 maxCapacity, uint32 storagePeriod, uint128 payment, uint8 minProviders) → uint64`    | `create_drive`       |
| `deleteDrive(uint64 driveId)`                                                                                         | `delete_drive`       |
| `shareDrive(uint64 driveId, bytes32 member, uint8 role)`                                                              | `share_drive`        |
| `unshareDrive(uint64 driveId, bytes32 member)`                                                                        | `unshare_drive`      |

### `IS3Registry` (s3-registry, `0x…09030000`)

| Selector                                                                                                                                 | Underlying extrinsic                |
| ---------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------- |
| `createS3Bucket(string name, uint32 minProviders) → uint64`                                                                              | `create_s3_bucket`                  |
| `createS3BucketWithStorage(string name, uint64 maxCapacity, uint32 duration, uint128 maxPayment) → uint64`                               | `create_s3_bucket_with_storage`     |
| `deleteS3Bucket(uint64 s3BucketId)`                                                                                                      | `delete_s3_bucket`                  |
| `putObjectMetadata(uint64 s3BucketId, string key, bytes32 cid, uint64 size, string contentType)`                                         | `put_object_metadata` (no user metadata in v1) |
| `deleteObjectMetadata(uint64 s3BucketId, string key)`                                                                                    | `delete_object_metadata`            |
| `copyObjectMetadata(uint64 srcBucketId, string srcKey, uint64 dstBucketId, string dstKey)`                                               | `copy_object_metadata`              |

### Type encoding rules

| Substrate          | Solidity   | Notes                                                                                                                              |
| ------------------ | ---------- | ---------------------------------------------------------------------------------------------------------------------------------- |
| `BucketId` / `DriveId` (`u64`) | `uint64`   | Synthesized pre-dispatch from `NextBucketId` / `NextDriveId` so the contract gets back the id assigned to *this* call.            |
| `BlockNumberFor<T>` (`u32`)    | `uint32`   |                                                                                                                                    |
| `BalanceOf<T>` (`u128`)        | `uint128`  | Substrate atomic units — not eth wei.                                                                                              |
| `AccountId` (`AccountId32`)    | `bytes32`  | Raw 32-byte sr25519 pubkey. Substrate-only providers can't be safely round-tripped through `AccountId32Mapper.to_address()`.       |
| `Role`             | `uint8`    | 0 = Admin, 1 = Writer, 2 = Reader.                                                                                                 |
| `EndAction`        | (split)    | `endAgreementPay` / `endAgreementBurn(uint8 burnPercent)` — cleaner Solidity than encoding the Rust enum directly.                 |
| `Option<Vec<u8>>` (drive name) | `string`   | Empty string ⇒ `None`.                                                                                                             |
| `Option<u8>` (`min_providers`) | `uint8`    | `0` ⇒ `None` (use runtime default); `1..=255` ⇒ `Some(n)`.                                                                          |

## Payment flow

`pallet_revive` charges the **caller's substrate-mapped account** for storage deposits, fees, and any `T::Currency::reserve` the dispatched extrinsic performs. For paid agreements, this means:

1. User sends `msg.value` (eth-side units) to the contract via a `payable` function.
2. `msg.value` lands in the contract's substrate-mapped account, automatically converted: `wei / NativeToEthRatio = substrate atomic units`. Our `NativeToEthRatio = 10^6` (Eth's 18 decimals minus our 12), so `1 UNIT` substrate = `10^18` wei.
3. The contract calls the precompile (e.g. `createBucketWithStorage`).
4. The precompile dispatches `pallet_storage_provider::create_bucket_with_storage(Origin::Signed(contract_account), …)`.
5. The pallet computes `payment = maxBytes × duration × matched_price` and runs `T::Currency::reserve(&contract_account, payment)` against the contract's balance.

Side-effect: the **contract** owns the bucket on-chain. Per-user ownership ("which of my dApp users bought this") lives in the contract's own state (`mapping(uint64 => address) bucketOwner` in `StorageMarketplace.sol`).

## Origin model

- `env.caller()` returns `Origin<AccountId>` from `pallet_revive`. For a contract-to-contract call this is the immediate caller; for an EOA-initiated tx this is the substrate-mapped EOA.
- The precompile builds `RawOrigin::Signed(account).into()` and passes it to the pallet's dispatchable. No custom origins, no admin elevation — pure substrate `Signed`.
- The precompile is **stateless** (`HAS_CONTRACT_INFO = false`). It does not own storage; it only dispatches on behalf of its caller.

## Weight and metering

Each precompile branch starts with `env.charge(<Runtime as PalletConfig>::WeightInfo::extrinsic_name())?` — the weight of the underlying extrinsic, conservatively pre-charged. Per the `Precompile` trait doc: *"Pre-compiles are unmetered code. Hence they have to charge an appropriate amount of weight themselves."*

For extrinsics with parameterized weights (e.g. `end_agreement(a: u32)`), we pass `1` as a conservative bound — refining these with proper benchmarks is a follow-up.

## Adding a new selector

1. Add the function to `src/IWeb3Storage.sol` (or `src/IDriveRegistry.sol`) in the precompile crate.
2. Add a `match` arm in `src/lib.rs` for the new variant:
   - `env.charge(WeightInfo::extrinsic_name())?`,
   - decode args, mapping `bytes32` to `AccountId` via `decode_account::<Runtime>`,
   - dispatch with `RawOrigin::Signed(env.caller_account_id())`,
   - encode the return value via `SolValue::abi_encode()`.
3. Mirror the function declaration into `examples/contracts/IWeb3Storage.sol` (the vendored ABI) and any contract that calls it.
4. Add a happy-path exercise to `examples/papi/sc-coverage.js` — direct `Revive.call(precompileAddr, calldata)` plus an assertion on the resulting on-chain state.

## Testing

Four scripts cover the surface end-to-end:

- **`just sc-demo`** (`examples/papi/sc-flow.js`) — full marketplace dApp story via `StorageMarketplace.sol`: deploy, `buyStorage` with `msg.value`, off-chain upload/challenge round-trip, `endMyAgreement`. Asserts provider earned tokens + contract events fired.
- **`just sc-coverage`** (`examples/papi/sc-coverage.js`) — direct precompile invocations (no intermediate contract) for every selector across all three precompiles. Each call submits as a signed substrate tx whose `dest` is the precompile address; on success, the script asserts the underlying pallet's storage / event was updated.
- **`just sc-team-drive`** (`examples/papi/sc-team-drive.js`) — drive-registry dApp via `SharedTeamDrive.sol`: deploy, `createTeam`, `invite` / `kick`, `disband`.
- **`just sc-token-gated`** (`examples/papi/sc-token-gated.js`) — s3-registry dApp via `TokenGatedDrive.sol`: deploy, `initialize`, `mint` an NFT-shaped access token per S3 object, `transfer`, `burn` (deletes object metadata), `shutdown`.

All four run in CI under `.github/workflows/integration-tests.yml` against both runtime matrix entries.

## Limits and follow-ups

In v1:

- **Provider-side selectors are not exposed.** `register_provider`, `add_stake`, `accept_agreement`, `respond_to_challenge`, `confirm_replica_sync`, `claim_checkpoint_rewards`, `fund_checkpoint_pool`, `deregister_*` all stay native — a dApp is a *user* of storage, not a provider.
- **Checkpoint extrinsics are not exposed.** `checkpoint` / `provider_checkpoint` take `BucketSnapshot` + `MmrProof` + `Vec<Signature>` — the ABI design needs a follow-up.
- **`challenge_offchain` is not exposed** (its `MerkleProof` argument needs the same treatment).
- **S3 `put_object_metadata` drops user metadata in v1.** The Rust extrinsic takes `Vec<(Vec<u8>, Vec<u8>)>`; the Solidity ABI for nested dynamic-bytes tuples is awkward, so the precompile selector accepts no user metadata and always passes an empty vector. Use the substrate extrinsic directly if you need it.
- **No `pallet_revive_eth_rpc` server.** PAPI drives revive directly via the substrate-native dispatchables (`call`, `instantiate_with_code`). A separate "real dApp UX" follow-up can ship the JSON-RPC node for MetaMask / viem / hardhat compatibility.
- **No benchmarks for the precompile crates.** Weights borrow the underlying extrinsic's weight info.
