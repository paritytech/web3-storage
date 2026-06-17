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

The contract is **part of the app**, not a shared example: its source lives at
`user-interfaces/photos/contracts/Photos.sol`, with the `IWeb3Storage.sol` interface it imports
vendored alongside it so the app is self-contained. It compiles (via `resolc`, like
`examples/contracts/build.sh`) to an ABI + bytecode artifact at
`user-interfaces/photos/src/contract/Photos.json`, which both the headless deploy recipe and the UI
import directly (the UI needs the ABI for viem encode/decode anyway).

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
    { "cid": "0x…", "thumbCid": "0x…", "name": "beach.jpg", "mime": "image/jpeg", "size": 1843200, "time": 1700000000 }
  ] }
```

- **Thumbnails**: each entry carries a `thumbCid` — a small, downscaled JPEG (e.g. longest edge ~320px)
  generated client-side at upload time and stored as its own blob, exactly like the full photo. The grid
  renders from `thumbCid` so listing a library downloads kilobytes per photo, not megabytes; the full
  `cid` is fetched only when a photo is opened. The thumbnail is itself content-addressed, so it's synced
  to every primary and copied on a switch alongside the full blob.
- **Integrity anchor**: the UI verifies `blake2-256(manifestBytes) == manifestCid` (the value the
  contract stored) before trusting a manifest fetched from a provider. Demonstrable security property.
  (Thumbnails are a UX optimization, not a security anchor — they're verified by their own CID like any
  blob, but a wrong thumbnail only mis-renders a cell; the full photo is always re-fetched by its `cid`.)
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
1. Generate a downscaled thumbnail in the browser (canvas → JPEG, longest edge ~320px).
2. `putBlob(photo) → cid` and `putBlob(thumb) → thumbCid` to **every** current primary (resolve endpoints
   via `resolveProviderEndpoint`).
3. Fetch + verify current manifest, append `{cid, thumbCid, name, mime, size, time}`, serialize.
4. `putBlob(manifest) → manifestCid` to every primary.
5. `setManifest(manifestCid)` — one cheap tx.

**Download / list**
- List: `getBlob(manifestCid)`, verify hash, parse, render the grid from each entry's `thumbCid`
  (kilobytes per cell).
- Download: open a photo → `getBlob(cid)` (full resolution) from any available primary.

**Switch providers**
1. User picks provider B → `addProvider(B, …){value}` → poll until B active.
2. Copy all photo blobs + manifest to B.
3. `dropProvider(A)` → A paid out and removed from `primary_providers`.

## Front-end app

A sibling app `user-interfaces/photos/`, matching the React 19 + Vite + Tailwind + PAPI stack and the
shared packages (`@web3-storage/{network-config,network-picker,papi}`).

| Concern | Choice |
| --- | --- |
| Dev port | **5178** (console 5173, drive 5174, provider 5175) |
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

Deployed **once per network**; the UI never asks a user to deploy. Everything contract-related — source,
build, and deploy — lives **inside the app**, so Photos is self-contained:
- **Source & build**: `user-interfaces/photos/contracts/{Photos.sol,IWeb3Storage.sol}` compiled with
  `resolc` to `user-interfaces/photos/src/contract/Photos.json` (abi + bin). A package script
  (e.g. `pnpm --filter photos build:contract`) produces it; both the deploy script and the UI import
  `Photos.json` directly.
- Add an optional `photosContract?: string` (H160) to `NetworkConfig`
  (`user-interfaces/shared/network-config/src/types.ts`), populated per network.
- **Deploy**: a **TypeScript** deploy script lives in the app at
  `user-interfaces/photos/scripts/deploy-contract.ts` (run via `tsx`; PAPI `Revive.instantiate_with_code`,
  reading bin from `Photos.json`). It can share the app's own TS deploy/encode helpers with the UI. A
  `just deploy-photos` recipe just invokes it, then injects the resulting address (reusing the
  landing-page injection mechanism, `landing/inject-config.mjs`).
- **Fallback**: a dev-only "Deploy contract" affordance in the UI when no address is configured (deploys
  the same `Photos.json` bin directly from the browser via `Revive.instantiate_with_code`).

## Integration points

- **Landing page** (`user-interfaces/landing/index.html`): add a 4th `<a class="card" data-app="photos" …>` card and a `'photos': './photos/'` entry in `BASES` (~lines 207–232).
- **Workspace**: add `photos` to `user-interfaces/pnpm-workspace.yaml`.
- **CI**: add to the build matrix in `ui-checks.yml` and build+assemble steps in `deploy-ui.yml` (`dist → _site/photos`, `404.html`).
- **Descriptors**: reuse the `Revive`-inclusive PAPI descriptors (as `examples/papi` uses).

## Testing

- **Integration** (the headless source of truth, mirroring `just sc-demo`): deploy `Photos` →
  `createLibrary(chosenProvider)` → poll accept → `putBlob(photo)` + manifest → `setManifest` →
  `addProvider(B)` → copy blobs → `dropProvider(A)` → assert library state, provider set, and payout.
  Add as a **TypeScript** flow in the app — `user-interfaces/photos/scripts/photos-flow.ts` (run via
  `tsx`) — reusing the same app-local TS helpers (`Photos.json` ABI, `openAgreement`, blob layer) the UI
  uses, plus a `just photos-flow` recipe.
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

## Implementation milestones

Built as **minimal, independently reviewable milestones**. The strategy is to prove the entire backend
headless first (contract → blobs → switch), because that's where the risk lives (precompile origin,
`msg.value`→payment, account mapping, async accept); only then build UI on a foundation that already
works. All contract source, build, and deploy/flow scripts are **TypeScript and live in the app**
(`user-interfaces/photos/`), not in `examples/`.

### M1 — `Photos.sol` + deploy + `createLibrary` (headless)

The riskiest seam, isolated. Vendor `contracts/{Photos.sol,IWeb3Storage.sol}` in the app; compile via
`resolc` to `src/contract/Photos.json`. TS deploy script `scripts/deploy-contract.ts` + `just
deploy-photos`. Headless: `ensureAccountMapped` → deploy → `createLibrary(provider, maxBytes, duration,
maxPayment){value}` → poll `bucket.primary_providers` until active → read back `libraryOf` unsigned
(`ReviveApi.call` + viem `decodeFunctionResult`).
**Done:** bucket exists, owned by the contract account, chosen provider auto-accepted, `libraryOf.exists`;
payment math verified against the provider's on-chain `price_per_byte` (`NativeToEthRatio = 10^6`).

### M2 — Blob layer + manifest + integrity anchor (headless)

`putBlob(bytes)→cid` / `getBlob(cid)→bytes` over the raw `/node` + `/commit` + `GET /node?hash=` API
(lift chunking from `examples/papi/api.js`, ported to app-local TS), giving per-photo root CIDs.
Manifest serialize (with the `thumbCid` field) + `blake2b256` CID + verify. Extend the flow:
`putBlob(photo)` → `putBlob(thumb)` → `putBlob(manifest)` → `setManifest(cid)` → re-fetch, assert
`blake2-256(bytes) == on-chain manifestCid`, byte-compare a downloaded photo. Real canvas downscaling is
browser-only, so the headless flow uses a placeholder thumb blob (e.g. a fixed small byte string) purely
to exercise the schema; actual thumbnail generation lands in M6. **Done:** round-trip a real multi-MB
photo; tamper check rejects a mutated manifest.

### M3 — Full headless switch flow → CI source of truth

Complete `scripts/photos-flow.ts` + `just photos-flow` (mirrors `just sc-demo`): `addProvider(B){value}`
→ poll B active → copy all blobs + manifest to B → `dropProvider(A)` → assert `primary_providers` and
A's payout. All agreement mechanics wrapped in the single `openAgreement(provider, terms)` helper (the
#97 seam). **Done:** one command runs deploy → create → upload → switch → assert against a local
chain+provider. The entire backend is now proven with zero UI.

### M4 — UI skeleton (state detection only)

Scaffold `user-interfaces/photos/` mirroring `provider/` (React 19 + Vite + Tailwind + PAPI; dev-accounts
+ extension wallet; `viem` dep; base `/web3-storage/photos/`; port **5178**). Plumbing: add to
`pnpm-workspace.yaml`, `run-local-uis`, landing card + `BASES`, `ui-checks.yml` matrix, `deploy-ui.yml`
assemble; add `photosContract?: H160` to `NetworkConfig`. App reads `libraryOf` unsigned and renders
**State A vs State B**. **Done:** runs locally on 5178, connects a dev account, shows "no library" vs
"bucket #N". No writes.

### M5 — State A in UI: create library

Provider list from `StorageProvider.Providers` (price/capacity/accepting); size/duration inputs; payment
compute + buffer with `value` in **substrate atomic units** (labeled in tokens); idempotent
`Revive.map_account()` before first write; `createLibrary` via `Revive.call` `signSubmitAndWatch`; poll
to State B. Reuses the M3 `openAgreement` helper. Surfaces `PaymentExceedsMax` and accept-timeout
clearly. **Done:** a fresh account goes A→B in the browser.

### M6 — State B in UI: upload + list + download

Port the M2 blob layer to the browser. Upload: generate a downscaled thumbnail (canvas → JPEG, longest
edge ~320px), `putBlob` both the full photo and the thumb to **every** current primary (endpoints via
`resolveProviderEndpoint`), append the `{cid, thumbCid, …}` manifest entry, `setManifest` last. List:
fetch manifest, verify hash, render the grid from `thumbCid` (kilobytes per cell); fetch the full `cid`
only when a photo is opened. Per-provider sync status + idempotent retry on partial upload. **Done:**
upload several photos, reload, grid renders from thumbnails, opening one downloads full-res.

### M7 — Headline: pick & switch providers in UI

Show current primaries from `bucket.primary_providers`; **+ Add** (pick B → `addProvider{value}` → poll
→ copy all blobs+manifest to B) and **Drop/Switch** (`dropProvider(A)`) with a guard blocking "drop your
only provider" + explicit confirmation. **Done:** add a second provider, watch sync, drop the first,
confirm photos still load from the survivor.

### M8 — Tests + polish

Playwright e2e (three states + upload + switch) via `@web3-storage/test-helpers`; dev-only "Deploy
contract" fallback when `photosContract` is unset; final error/edge pass (unmapped account, accept
timeout, manifest tamper, partial upload). Optional Solidity unit tests if a harness is added.

## Open questions / follow-ups

- **Albums** - nest the manifest.
- **Client-side image editing** (crop/rotate) — feasible as copy-on-write (new CID + `setManifest`); superseded versions accumulate within quota (no GC). See *Data mutability & editing*.
- **Library deletion** (`endLibrary`) ending all agreements + freezing the bucket.
