# Photos — decentralized photo storage dApp (prototype)

## Goal

A fourth app under `user-interfaces/` plus a landing-page card, demonstrating a custom Solidity
contract (`Photos.sol`) that drives Web3 Storage through the storage-provider precompile, walking a
signed-in user through:

1. **No library** — the user hasn't set up storage yet; let them create one *with a provider they choose*.
2. **Has library** — the user can upload/download photos.
3. **Pick & switch providers** — the headline: add another chosen provider, copy photos to it, drop one
   you don't want. Your memories aren't hostage to a single provider.

## Scope decisions (locked)

| Decision | Choice | Rationale |
| --- | --- | --- |
| Transport / signing | **Substrate-native only** (Polkadot extension + dev accounts) | Off-chain provider auth keeps working; no extra infra. |
| Contract calls | **PAPI `Revive` dispatchables** (`call`, `instantiate_with_code`), **viem for ABI only** | The CI-verified [`sc-api.js`](../../examples/papi/sc-api.js) pattern. |
| EVM JSON-RPC / MetaMask | **Out of scope** | Runtime is eth-rpc-ready (`runtime/src/revive.rs`), so a MetaMask UX is a clean future follow-up. |
| Architecture | **Layer 0 + custom `Photos` contract, contract-owned bucket per user** | Single precompile (`0x…0901`); the contract does real storage work — the cleanest "contract *integrated with* storage" example. |
| Provider model | **Multiple user-chosen PRIMARY providers** per bucket (up to 5) | Fully contract-orchestrated; delivers "pick & switch". Protocol replicas/self-hosting are native-only → out of scope. |
| Photo index | **On-chain manifest-root CID + off-chain manifest blob** | Cheap on-chain, scales, content-addressed; on-chain CID is an integrity anchor. |
| Library structure (v1) | **Flat timeline** (no albums) | Albums = a later nesting of the manifest. |

### Why not Layer 1 (drives)?

The drive registry **auto-selects** providers (`query_available_providers(...)[0]`, no ranking; see
`pallet/src/lib.rs:4485`) and exposes no add/switch-provider call — it abstracts away exactly our
headline feature. Its multi-provider shape is *1 auto-picked primary + auto-picked replicas*, not
user-chosen primaries. So Photos is built on Layer 0.

## Architecture

The contract is the per-user **control plane** (bucket + provider lifecycle + manifest pointer). Photo
blobs and the manifest live off-chain on the chosen providers, content-addressed by blake2-256.

```
Photos UI (React · dev-account/extension wallet · viem for ABI)
   │  PAPI: Revive.call (writes) · ReviveApi.call (unsigned reads)
   ▼
Photos.sol (PolkaVM)            per user: { bucketId, manifestCid }
   │  CALL 0x…09010000 (storage-provider precompile)
   ▼
pallet_storage_provider         bucket owned by the contract account · up to 5 chosen primaries
   │
   └─►  provider nodes (HTTP /node)   hold the photo blobs + the manifest blob
        (off-chain, browser ↔ provider, bypasses the contract)
```

Origin model (from [`smart-contracts.md`](./smart-contracts.md)): precompile calls dispatch as
`RawOrigin::Signed(contract_account)`, so the **contract** owns every user's bucket. Per-user
attribution lives in the contract (`bucketOwner`). This is custodial-by-ownership only — the
transparent contract enforces "only you manage your library."

## The `Photos` contract

Calls only the storage-provider precompile (`IWeb3Storage`, `0x…09010000`).

```solidity
contract Photos {
    IWeb3Storage constant WEB3 = IWeb3Storage(0x0000000000000000000000000000000009010000);

    struct Library { uint64 bucketId; bytes32 manifestCid; bool exists; }
    mapping(address => Library) public libraries;    // user → their library
    mapping(uint64 => address)  public bucketOwner;  // ownership guard

    event LibraryCreated (address indexed user, uint64 indexed bucketId, bytes32 provider);
    event ProviderAdded  (address indexed user, uint64 indexed bucketId, bytes32 provider);
    event ProviderDropped (address indexed user, uint64 indexed bucketId, bytes32 provider);
    event ManifestUpdated(address indexed user, bytes32 manifestCid);

    /// Create my library with a provider I chose. `msg.value` funds the agreement payment,
    /// reserved from the contract's balance when the precompile dispatches.
    function createLibrary(bytes32 provider, uint64 maxBytes, uint32 duration, uint128 maxPayment)
        external payable returns (uint64 bucketId)
    {
        require(!libraries[msg.sender].exists, "library exists");
        bucketId = WEB3.createBucket(1);
        WEB3.requestPrimaryAgreement(bucketId, provider, maxBytes, duration, maxPayment);
        libraries[msg.sender] = Library(bucketId, bytes32(0), true);
        bucketOwner[bucketId] = msg.sender;
        emit LibraryCreated(msg.sender, bucketId, provider);
    }

    /// Add another chosen primary to my bucket (redundancy / the first half of a switch).
    function addProvider(bytes32 provider, uint64 maxBytes, uint32 duration, uint128 maxPayment)
        external payable
    {
        Library storage lib = libraries[msg.sender];
        require(lib.exists, "no library");
        WEB3.requestPrimaryAgreement(lib.bucketId, provider, maxBytes, duration, maxPayment);
        emit ProviderAdded(msg.sender, lib.bucketId, provider);
    }

    /// Drop a provider (second half of a switch). Pays them out in full.
    function dropProvider(bytes32 provider) external {
        Library storage lib = libraries[msg.sender];
        require(lib.exists, "no library");
        WEB3.endAgreementPay(lib.bucketId, provider);
        emit ProviderDropped(msg.sender, lib.bucketId, provider);
    }

    /// Update the manifest root after the client uploaded photos + the new manifest off-chain.
    function setManifest(bytes32 manifestCid) external {
        Library storage lib = libraries[msg.sender];
        require(lib.exists, "no library");
        lib.manifestCid = manifestCid;
        emit ManifestUpdated(msg.sender, manifestCid);
    }

    /// UI reads this unsigned via `ReviveApi.call` (no signature, no gas) for state detection.
    function libraryOf(address user)
        external view returns (uint64 bucketId, bytes32 manifestCid, bool exists)
    {
        Library memory l = libraries[user];
        return (l.bucketId, l.manifestCid, l.exists);
    }
}
```

Notes:
- The **current provider set** is not stored in the contract — it's already on-chain in
  `bucket.primary_providers`, which the UI reads directly from `StorageProvider.Buckets`.
- `bytes32 provider` is the substrate `AccountId32` (raw 32-byte sr25519 pubkey), per the precompile's
  type-encoding rules.
- `maxBytes`/`duration`/`maxPayment` mirror `requestPrimaryAgreement`; the contract is the bucket admin
  (it created the bucket), so `ensure_admin` passes.
- Precompile selectors used: `createBucket`, `requestPrimaryAgreement`, `endAgreementPay` (all already
  exposed in `IWeb3Storage.sol`). **No precompile changes needed.**

### Payment

`payment = price_per_byte × maxBytes × duration`, where `price_per_byte` is read from the chosen
provider's on-chain settings. `msg.value` (eth-side) funds the contract's substrate-mapped account;
`pallet_revive` converts at `NativeToEthRatio = 10^6`. The precompile then reserves the payment from
the contract's balance. The UI sets `msg.value` from the computed payment plus a buffer. Unused
reserve stays in the contract in v1 (per-user refunds = a follow-up; acceptable for a prototype).

## Manifest & blob layer

The photo list is a manifest **blob** stored in the bucket like any photo; its blake2-256 CID is the
`manifestCid` recorded on-chain.

```json
{ "version": 1,
  "photos": [
    { "cid": "0x…", "name": "beach.jpg", "mime": "image/jpeg", "size": 1843200, "time": 1700000000 }
  ] }
```

- **Integrity anchor**: the UI verifies `blake2-256(manifestBytes) == manifestCid` (the value the
  contract stored) before trusting a manifest fetched from a provider. Demonstrable security property.
- **Blob layer** — a thin `putBlob(bytes) → cid` / `getBlob(cid) → bytes` over the provider's Layer-0
  HTTP API (`PUT /node` + `POST /commit`, `GET /node?hash=…`), handling chunking for multi-MB photos.
  Reuse the existing TS file-system SDK's chunking (`user-interfaces/sdk/typescript/file-system-sdk`)
  rather than reinventing it; each photo's `cid` is its per-photo root.
- v1 manifest is a flat, time-sorted list. Albums would nest entries under directory keys later.

## Data mutability & editing

Storage is **copy-on-write**: blobs are immutable (content-addressed by blake2-256, committed to an
append-only MMR — `provider-node/src/storage/in_memory.rs`). You never edit bytes in place; you write a
new blob (new CID) and repoint the reference. That *is* the manifest pattern: `setManifest` repoints
`manifestCid`. **Client-side image editing (e.g. crop) fits the same model** — crop in the browser,
`putBlob` the result as a new photo CID, update the manifest entry, `setManifest`. The original lingers.

Implications:
- **No garbage collection.** Superseded blobs (old manifest versions, pre-edit photos) are never
  reclaimed; they persist for the agreement's life. There is an admin MMR-prune (`POST /delete` →
  `delete_before`, by sequence) and the `Deleted` challenge response, but these prune historical leaves,
  don't free chunks on disk, and aren't per-blob deletes. S3/FS deletes only drop the key→CID mapping.
- **Quota = total of all versions.** An agreement pays for `max_bytes × duration` up front (not metered
  per-use); accumulated versions consume that quota. To grow it, `topUpAgreement` (pay
  `additional_bytes × remaining_duration × price`). Budget for the sum of all versions, not just current.

## Provider model & the switch flow

- **Auto-accept**: `requestPrimaryAgreement` creates an *agreement request*; the chosen provider's node
  auto-accepts within a few blocks (`provider-node/src/agreement_coordinator.rs`, defaults
  `auto_accept=true`, `accepting_primary=true`). The agreement is not active until then — see async
  handling below.
- **Up to 5 primaries** per bucket (`MaxPrimaryProviders = 5`). All primaries hold the full data, so
  the client uploads each photo + the manifest to **every** current primary.
- **Switch** = `addProvider(B)` → copy all blobs+manifest to B → `dropProvider(A)`. Overlap while you
  evaluate; the index is untouched (same CIDs everywhere).

## Data flows

**Create library (State A → B)**
1. UI lists providers from `StorageProvider.Providers` (price, capacity, accepting). User picks one.
2. Ensure `Revive.map_account()` for the user (once).
3. `createLibrary(provider, maxBytes, duration, maxPayment)` with `value` = payment + buffer.
4. Poll `bucket.primary_providers` until the chosen provider is active (it auto-accepted), then → State B.

**Upload a photo**
1. `putBlob(photo) → cid` to **every** current primary (resolve endpoints via `resolveProviderEndpoint`).
2. Fetch + verify current manifest, append `{cid, name, mime, size, time}`, serialize.
3. `putBlob(manifest) → manifestCid` to every primary.
4. `setManifest(manifestCid)` — one cheap tx.

**Download / list**
- List: `getBlob(manifestCid)`, verify hash, parse, render the grid (thumbnails by downloading each `cid`).
- Download: `getBlob(cid)` from any available primary.

**Switch providers**
1. User picks provider B → `addProvider(B, …){value}` → poll until B active.
2. Copy all photo blobs + manifest to B.
3. `dropProvider(A)` → A paid out and removed from `primary_providers`.

## Front-end app

A sibling app `user-interfaces/photos/`, matching the React 19 + Vite + Tailwind + PAPI stack and the
shared packages (`@web3-storage/{network-config,network-picker,papi}`).

| Concern | Choice |
| --- | --- |
| Dev port | **5176** (console 5173, drive 5174, provider 5175) |
| Wallet | Dev accounts (zero-setup) **and** Polkadot extension, like the provider UI |
| New dep | `viem` (ABI encode/decode only) |
| Reads | `ReviveApi.call` dry-run + viem `decodeFunctionResult` (unsigned) |
| Writes | `Revive.call` / `Revive.instantiate_with_code` via PAPI `signSubmitAndWatch` |
| Base | `GITHUB_PAGES` base `/web3-storage/photos/` |

### Screens (single-page, state-driven)

```
┌────────────────────────────────────────────────────────────┐
│  Photos · Web3 Storage          [network ▾] [wallet ▾]       │
├────────────────────────────────────────────────────────────┤
│  STATE A — no library                                        │
│   Pick a provider:  ▢ alice (1/GB) ▢ bob (2/GB) …            │
│   [ size ] [ duration ]            ( Create library )        │
│                                                              │
│  STATE B — library (bucket #N)                               │
│   Providers: alice ●  bob ●   ( + Add )  ( Switch… )         │
│   ┌───┬───┬───┐                                              │
│   │img│img│img│   …photo grid (from manifest)                │
│   └───┴───┴───┘   ( Upload )                                 │
└────────────────────────────────────────────────────────────┘
```

- **Value units**: `Revive.call`'s `value` is **substrate atomic units**, not wei — label the buy
  amount in tokens and pass atomic units directly.

## Error handling & edge cases

- **Unmapped account** → prompt/run `Revive.map_account()` before the first write (idempotent).
- **Async accept** → after `createLibrary`/`addProvider`, poll `bucket.primary_providers` until the
  chosen provider appears; timeout with a clear message if it never accepts (not accepting / no capacity).
- **Insufficient `msg.value`** → compute payment from the provider's `price_per_byte` and add a buffer;
  surface `PaymentExceedsMax` clearly.
- **Partial multi-provider upload** → upload to all current primaries; show per-provider sync status and
  retry failures. `setManifest` is last; blobs are content-addressed, so retrying is idempotent.
- **Manifest tampering** → reject a fetched manifest whose hash ≠ on-chain `manifestCid`.
- **Dropping your only provider** → block / require explicit confirmation (you'd lose access).

## Contract deployment

Deployed **once per network**; the UI never asks a user to deploy.
- Add an optional `photosContract?: string` (H160) to `NetworkConfig`
  (`user-interfaces/shared/network-config/src/types.ts`), populated per network.
- **Local dev**: a `just deploy-photos` recipe deploys `Photos` via the `deployContract()` helper and
  injects the address (reusing the landing-page injection mechanism, `landing/inject-config.mjs`).
- **Fallback**: a dev-only "Deploy contract" affordance when no address is configured.

## Integration points

- **Landing page** (`user-interfaces/landing/index.html`): add a 4th `<a class="card" data-app="photos" …>` card and a `'photos': './photos/'` entry in `BASES` (~lines 207–232).
- **Workspace**: add `photos` to `user-interfaces/pnpm-workspace.yaml`.
- **CI**: add to the build matrix in `ui-checks.yml` and build+assemble steps in `deploy-ui.yml` (`dist → _site/photos`, `404.html`).
- **Descriptors**: reuse the `Revive`-inclusive PAPI descriptors (as `examples/papi` uses).

## Testing

- **Integration** (the headless source of truth, mirroring `just sc-demo`): deploy `Photos` →
  `createLibrary(chosenProvider)` → poll accept → `putBlob(photo)` + manifest → `setManifest` →
  `addProvider(B)` → copy blobs → `dropProvider(A)` → assert library state, provider set, and payout.
  Add as `examples/papi/photos-flow.js` + a `just` recipe.
- **UI e2e** (Playwright + `@web3-storage/test-helpers`): the three states + upload + switch.
- **Contract**: covered by the integration script; optional Solidity unit tests if a harness is added.

## Forward-compatibility with issue #97

[#97](https://github.com/paritytech/web3-storage/issues/97) will move agreement negotiation off-chain:
it removes `create_bucket(_with_storage)`, `request_primary_agreement` + `accept_agreement`, and
`find_matching_provider`, replacing them with a single **`establish_agreement(bucket_id, provider,
terms, provider_signature)`** — the provider signs the deal terms off-chain (HTTP), the client submits
that signature on-chain. The auto-accept loop then disappears (the signature proves consent
synchronously). #97 is open/unimplemented; #133 builds on the **current** API but isolates the seam so
the migration is localized:

- **Contract seam**: the only precompile-agreement calls live in `createLibrary` (createBucket +
  requestPrimaryAgreement) and `addProvider` (requestPrimaryAgreement). Under #97 these collapse to a
  single `establishAgreement(provider, terms, signature)` precompile selector (which the precompile
  would expose, coordinated with #97). Keep agreement calls *only* in these two functions — don't
  spread them — so the change is one selector swap per call site.
- **Client seam**: wrap agreement creation in one helper `openAgreement(provider, terms)`. Today it
  calls the contract and polls for auto-accept; under #97 it first fetches the provider's signature
  over the terms via HTTP, then calls the contract. UI code only ever calls the helper.
- **What drops out under #97**: the async-accept polling step, and the `createBucket` selector.

This keeps the prototype shippable now while making the #97 cutover a small, well-located edit. It does
**not** change the Layer 0 decision — #97 alters agreement *mechanics*, not the drive registry's
provider choice/switching, so Layer 1 remains unsuitable for the headline feature.

## Open questions / follow-ups

- **Albums** - nest the manifest.
- **Thumbnails** - store a downscaled blob per photo for fast grids.
- **Client-side image editing** (crop/rotate) — feasible as copy-on-write (new CID + `setManifest`); superseded versions accumulate within quota (no GC). See *Data mutability & editing*.
- **Library deletion** (`endLibrary`) ending all agreements + freezing the bucket.
