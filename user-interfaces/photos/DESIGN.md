# Photos — decentralized photo storage dApp (prototype)

## Goal

Photos is a normal photo app backed by Web3 Storage. A custom Solidity contract
(`Photos.sol`) creates a Layer 0 bucket through the storage-provider precompile (`IWeb3Storage`),
and the app stores a Layer 1 file system in that bucket through the SDK's `FileSystemClient`,
walking a signed-in user through:

1. **No library** — the user hasn't set up storage yet; let them create one *with a provider they
   choose*.
2. **Has library** — the user can organize photos into **albums** (folders), upload, view,
   **edit**, and download them.

The point of the app is to show a familiar product experience (albums, a photo grid, a
lightbox, in-browser editing) running entirely on decentralized storage — with a custom contract
as the on-chain control plane.

## Scope decisions (locked)

| Decision | Choice | Rationale |
| --- | --- | --- |
| Transport / signing | **Substrate-native only** (Polkadot extension + dev accounts) | Off-chain provider auth keeps working; no extra infra. |
| Storage layer | **Layer 0 bucket + the SDK's Layer 1 `FileSystemClient`** | Directories give us **albums for free**, and the SDK's `FileSystemClient` is reusable. |
| Contract calls | **PAPI `Revive` dispatchables** (`call`, `instantiate_with_code`), **viem for ABI only** | The CI-verified [`sc-coverage.ts`](../../examples/papi/sc-coverage.ts) / [`sc-team-drive.ts`](../../examples/papi/sc-team-drive.ts) pattern. |
| EVM JSON-RPC / MetaMask | **Out of scope** | Runtime is eth-rpc-ready (`runtimes/web3-storage-local/src/revive.rs`), so a MetaMask UX is a clean future follow-up. |
| Architecture | **Custom `Photos` contract, one contract-administered bucket per user** | The contract creates the bucket via the **storage-provider precompile** (`IWeb3Storage`, `0x…09010000`), is its admin, and anchors the album-tree root on-chain — a job the chain itself does not do. |
| Provider model | **Single user-chosen primary provider** per bucket | `create_bucket_with_primary` opens a bucket + one primary atomically; the user picks the provider at creation. |
| Album/tree state | **Directory tree stored in the bucket + on-chain root anchor in the contract** | The client writes the tree as blobs in the bucket; the contract stores a **client-computed** `metadata_merkle_root` as an integrity anchor. |
| Library structure (v1) | **Albums = directories** (one level of folders) | Nested sub-albums are a later extension of the same directory model. |

### Why the file-system client (and where the contract fits)

The SDK's `FileSystemClient` gives a real **directory tree** per bucket, so albums and folders come
for free instead of being hand-rolled into a manifest. `create_bucket_with_primary` takes an
explicit `(provider, terms, signature)`, so the user **chooses** the provider.

The custom contract is the headline integration: the **storage-provider precompile**
(`IWeb3Storage`, `0x…09010000`) lets `Photos.sol` create a bucket on the user's behalf and
grant the user write access, the same pattern as
[`SharedTeamDrive.sol`](../../examples/contracts/SharedTeamDrive.sol). The contract also stores
each bucket's current album-tree root CID on-chain (`setRoot`), which the chain does not track.
This gives the contract a real job and the app a verifiable integrity property.

## Architecture

The contract is the per-user **control plane** (bucket creation + the on-chain root anchor).
Photo blobs and the album/directory tree live off-chain on the chosen provider,
content-addressed by blake2-256.

```
Photos UI (React · dev-account/extension wallet · viem for ABI)
   │  PAPI: Revive.call (writes) · ReviveApi.call (unsigned reads)
   │  HTTP: Layer 0 routes via FileSystemClient (albums, photos, thumbnails) — direct, bypasses the contract
   ▼
Photos.sol (PolkaVM)            per user: { bucketId, rootCid }
   │  CALL 0x…09010000 (storage-provider precompile, IWeb3Storage)
   ▼
pallet_storage_provider         bucket + one primary agreement · contract account is admin
   │                            · user granted Writer
   ▼
provider node  Layer 0 routes      holds the photo blobs, thumbnails, and the directory tree blobs
        (off-chain, browser ↔ provider; client-computed tree root anchored on-chain by the contract)
```

Origin model (from [`smart-contracts.md`](../../docs/drafts/smart-contracts.md)): precompile calls dispatch as
`RawOrigin::Signed(contract_account)`, so the **contract** is the admin of every user's bucket.
Per-user attribution lives in the contract (`bucketOwner`). At creation the contract grants the
user a **Writer** role on the bucket (`setMember` → `set_member` on the storage-provider
pallet), so the browser can sign the provider's Layer 0 write requests directly with the user's own
wallet. This is custodial-by-ownership only — the transparent contract enforces "only you manage
your library."

## The `Photos` contract

The contract is **part of the app**, not a shared example: its source lives at
`contracts/Photos.sol`, with the `IWeb3Storage.sol` interface it imports vendored alongside it so
the app is self-contained. It compiles (via `resolc`, like `examples/contracts/build.sh`) to an
ABI + bytecode artifact at `src/contract/Photos.json`, which both the headless deploy recipe and
the UI import directly (the UI needs the ABI for viem encode/decode anyway).

Calls only the storage-provider precompile (`IWeb3Storage`, `0x…09010000`).

```solidity
contract Photos {
    IWeb3Storage constant STORAGE = IWeb3Storage(0x0000000000000000000000000000000009010000);

    struct Library { uint64 bucketId; bytes32 rootCid; bool exists; }
    mapping(address => Library) public libraries;    // user → their library
    mapping(uint64 => address)  public bucketOwner;  // ownership guard

    event LibraryCreated(address indexed user, uint64 indexed bucketId, bytes32 provider);
    event RootUpdated   (address indexed user, uint64 indexed bucketId, bytes32 rootCid);

    /// Create my library with a provider I chose. `msg.value` funds the agreement payment,
    /// reserved from the contract's balance when the precompile dispatches. The contract is the
    /// bucket admin and grants me (`userAccount`, my substrate AccountId32) a Writer role so my
    /// browser can upload directly to the provider.
    function createLibrary(
        bytes32 userAccount,
        bytes32 provider,
        IWeb3Storage.PrimitiveAgreementTerms calldata terms,
        bytes calldata signature
    ) external payable returns (uint64 bucketId) {
        require(!libraries[msg.sender].exists, "library exists");
        require(!terms.hasBucketId, "primary terms must not be bucket-bound");
        bucketId = STORAGE.createBucketWithPrimary(
            provider, terms, signature, IWeb3Storage.Visibility.Private
        );
        STORAGE.setMember(bucketId, userAccount, IWeb3Storage.Role.Writer);
        libraries[msg.sender] = Library(bucketId, bytes32(0), true);
        bucketOwner[bucketId] = msg.sender;
        emit LibraryCreated(msg.sender, bucketId, provider);
    }

    /// Anchor the current album-tree root on-chain after the client changed the tree off-chain
    /// (upload / new album / edit / delete). `rootCid` is the metadata Merkle root the client
    /// computes over the bucket's sorted (path, data_root, size) entries.
    function setRoot(bytes32 rootCid) external {
        Library storage lib = libraries[msg.sender];
        require(lib.exists, "no library");
        lib.rootCid = rootCid;
        emit RootUpdated(msg.sender, lib.bucketId, rootCid);
    }

    /// UI reads this unsigned via `ReviveApi.call` (no signature, no gas) for state detection
    /// and to fetch the integrity anchor.
    function libraryOf(address user)
        external view returns (uint64 bucketId, bytes32 rootCid, bool exists)
    {
        Library memory l = libraries[user];
        return (l.bucketId, l.rootCid, l.exists);
    }
}
```

Notes:
- `bytes32 provider` / `bytes32 userAccount` are substrate `AccountId32`s (raw 32-byte account
  ids), per the precompile's type-encoding rules. `userAccount` is the signed-in user's own
  substrate account — the one their wallet signs provider write requests with.
- `terms` is the precompile's `PrimitiveAgreementTerms`; `terms.owner` must be the contract's
  substrate-mapped account (the bucket admin). For a primary agreement, `hasBucketId = false` and
  `hasReplicaParams = false`. `price_per_byte` comes from the provider's **signed** terms.
- Precompile selectors used: `createBucketWithPrimary`, `setMember`. `setRoot`/`libraryOf` are
  the contract's own state — the on-chain anchor that the chain doesn't provide. **No precompile
  changes needed.**
- The contract is the bucket admin (it created the bucket), so the `setMember` admin check
  passes. The bucket is created `Private`.
- The chain stores no library name; the UI shows the library as "Bucket #N".

### Payment

`payment = price_per_byte × max_bytes × duration`, where `price_per_byte` is the value the
provider locked into the **signed terms** (from its `/negotiate` response). `msg.value` (eth-side)
funds the contract's substrate-mapped account; `pallet_revive` converts at `NativeToEthRatio =
10^6`. `create_bucket_with_primary` then reserves the payment from the contract's balance via Layer 0. The UI
sets `msg.value` from the computed payment plus a buffer. Unused reserve stays in the contract in
v1 (per-user refunds = a follow-up; acceptable for a prototype).

## Albums, blobs & the root anchor

Layer 1 gives a real directory tree per bucket. The SDK's `FileSystemClient`
(`@web3-storage/sdk/fs`) stores it in the bucket itself: file content, `FileManifest` and
`DirectoryNode` blobs go up through the provider's Layer 0 routes (`PUT /node`, `POST /commit`),
and the bucket's last MMR leaf points at the root directory. Reads go through `GET /read` and
`GET /node`. Writes are signed by the user's wallet; reads are unauthenticated. The app calls
the provider only through `FileSystemClient` methods such as `createDirectory`, `listDirectory`,
`uploadFile`, and `downloadFile`.

- **Albums**: directories. v1 ships one level of folders (`/Beach`, `/Family`); the same model
  nests for sub-albums later.
- **Thumbnails**: each photo gets a small, downscaled JPEG (longest edge ~320px) generated
  client-side at upload time and stored as its own file under a parallel `.thumbs/` subtree
  (e.g. photo `/Beach/x.jpg` → thumb `/.thumbs/Beach/x.jpg`). The grid renders from thumbnails so
  listing an album downloads kilobytes per photo, not megabytes; the full file is fetched only
  when a photo is opened.
- **Integrity anchor (client-computed)**: the bucket's metadata root is a *deterministic*
  blake2-256 Merkle tree over the bucket's **sorted** `(path, data_root, size)` entries
  (`metadataMerkleRoot` in `packages/core/src/merkle.ts`), where each file's `data_root` is the content root the client
  already produces while chunking the upload. The client therefore **computes the root itself**
  rather than trusting the provider. After any mutation it anchors the locally-computed root via
  `setRoot(rootCid)`. To verify a library it recomputes the root from a fresh `ls` plus the
  downloaded files (checking each file against its own `data_root`) and asserts it equals the
  on-chain `rootCid` from `libraryOf` — a provider that hides, adds, swaps, or tampers with any
  file produces a mismatch. Thumbnails are stored as ordinary files, so they're covered by the same
  root.

## Data mutability & editing

Storage is **copy-on-write**: blobs are immutable (content-addressed by blake2-256, committed to
an append-only MMR — `crates/providers/storage/src/backend/rocksdb.rs`). You never edit bytes in place; a
`PUT` to a path writes a **new** blob (new CID) and repoints that path in the tree. The album
tree's root changes, so each mutation ends with a freshly **recomputed** root → `setRoot`.

**Client-side image editing** (crop, rotate, filters) fits the same model directly: edit in the
browser, `PUT` the result back (to the same path to replace, or a new path to keep both), then
recompute the root locally and `setRoot`. The pre-edit bytes linger as a superseded blob.

Implications:
- **No garbage collection.** Superseded blobs (pre-edit photos, replaced thumbnails) are never
  reclaimed; they persist for the agreement's life. FS deletes only write a new directory tree
  without the entry.
- **Quota = total of all versions.** An agreement pays for `max_bytes × duration` up front;
  accumulated versions consume that quota. To grow it, top up the agreement
  (`additional_bytes × remaining_duration × price`). Budget for the sum of all versions.

## Provider model

- **Single chosen primary.** `create_bucket_with_primary` opens one Layer 0 bucket + one primary agreement
  atomically; the user picks the provider at library creation. (Redundancy via protocol replicas
  is a native-only follow-up — see open questions.)
- **No auto-accept polling.** The provider signs the deal terms off-chain at `POST /negotiate`
  (`provider-node/src/api.rs`); the client redeems that signature on-chain via the contract's
  `createLibrary` → `createBucketWithPrimary`. The signature is synchronous consent, so the bucket is active
  as soon as the extrinsic is included — no waiting for the provider to accept.

## Data flows

**Create library (State A → B)**
1. UI lists providers from `StorageProvider.Providers` (price, capacity, accepting). User picks one.
2. Ensure `Revive.map_account()` for the user (once, idempotent).
3. `POST /negotiate` to the chosen provider for signed `terms` (owner = the contract's
   substrate-mapped account); shape them into `PrimitiveAgreementTerms` (reuse
   `negotiatePrecompileTerms`).
4. `createLibrary(userAccount, provider, terms, signature)` via `Revive.call` with
   `value` = payment + buffer. The bucket is active on inclusion; → State B.

**Create an album**
- `createDirectory` for the new folder → recompute the tree root locally → `setRoot`.

**Upload a photo**
1. Generate a downscaled thumbnail in the browser (canvas → JPEG, longest edge ~320px).
2. `uploadFile` to `/Album/photo.jpg` (full) and `/.thumbs/Album/photo.jpg` (thumb), keeping each
   file's locally-computed `data_root`.
3. Recompute the bucket's metadata Merkle root locally → `setRoot(rootCid)` — one cheap tx.

**Edit a photo**
- Crop/rotate in the browser → `PUT` the result (same path to replace, or a new path to keep
  both) → recompute the root locally → `setRoot`. Copy-on-write; the original lingers.

**List / view**
- List an album: `listDirectory(bucketId, '/Album')`, render the grid from each entry's thumbnail
  (kilobytes per cell); recompute the tree root locally from the listing (+ downloaded files) and
  check it equals the on-chain anchor.
- View: open a photo → `downloadFile` (full resolution) in a lightbox.

## Front-end app

This app matches the React 19 + Vite + Tailwind + PAPI stack and the shared packages
(`@web3-storage/{network-config,network-picker,papi}`) and the SDK's `FileSystemClient`.

| Concern | Choice |
| --- | --- |
| Dev port | **5178** (landing 5176, drive 5174, provider 5175, s3 5177, explorer 5179) |
| Wallet | Dev accounts (zero-setup) **and** Polkadot extension, like the provider UI |
| New dep | `viem` (ABI encode/decode only) |
| Reads | `ReviveApi.call` dry-run + viem `decodeFunctionResult` (unsigned) |
| Writes | `Revive.call` / `Revive.instantiate_with_code` via PAPI `createAndSubmit` |
| FS ops | the SDK's `FileSystemClient` over the provider's Layer 0 routes |
| Base | `GITHUB_PAGES` base `/web3-storage/photos/` |

### Screens (single-page, state-driven)

```
┌────────────────────────────────────────────────────────────┐
│  Photos · Web3 Storage          [network ▾] [wallet ▾]       │
├────────────────────────────────────────────────────────────┤
│  STATE A — no library                                        │
│   Pick a provider:  ● alice (1/GB)  ○ bob (2/GB) …           │
│   [ size ] [ duration ]            ( Create library )        │
│                                                              │
│  STATE B — library (Bucket #N · provider alice ●)            │
│   Albums:  [ All ] [ Beach ] [ Family ]   ( + New album )    │
│   ┌───┬───┬───┐                                              │
│   │img│img│img│   …photo grid (thumbnails)   ( Upload )      │
│   └───┴───┴───┘   click → lightbox ( Edit ) ( Download )     │
└────────────────────────────────────────────────────────────┘
```

- **Value units**: `Revive.call`'s `value` is **substrate atomic units**, not wei — label the buy
  amount in tokens and pass atomic units directly.

## Error handling & edge cases

- **Unmapped account** → prompt/run `Revive.map_account()` before the first write (idempotent).
- **Negotiate failure / expired terms** → if `/negotiate` fails or the quote's `valid_until` has
  passed, re-negotiate from scratch before retrying `createLibrary`; surface a clear message if
  the provider isn't accepting or has no capacity.
- **Insufficient `msg.value`** → compute payment from the signed `price_per_byte` and add a
  buffer; surface `PaymentExceedsMax` clearly.
- **Provider authorization** → the browser signs the provider's Layer 0 write requests with the
  user's wallet; the Writer role granted at `createLibrary` (`setMember`) makes them pass. Reads are
  unauthenticated.
- **Integrity mismatch** → reject/flag a library whose locally-recomputed metadata root ≠ the
  on-chain `rootCid`.
- **Upload retry** → `PUT` is idempotent for the same bytes (content-addressed); `setRoot` is the
  last step, so a retried upload re-anchors the same root.

## Contract deployment

Deployed **once per network**; the UI never asks a user to deploy. Everything contract-related —
source, build, and deploy — lives **inside the app**, so Photos is self-contained:
- **Source & build**: `contracts/{Photos.sol,IWeb3Storage.sol}` compiled with `resolc` to
  `src/contract/Photos.json` (abi + bin). A package script (`pnpm --filter @web3-storage/photos
  build:contract`) produces it; both the deploy script and the UI import `Photos.json` directly.
- Add an optional `photosContract?: string` (H160) to `NetworkConfig`
  (`user-interfaces/shared/network-config/src/types.ts`), populated per network.
- **Deploy**: a **TypeScript** deploy script lives in the app at `scripts/deploy-contract.ts`
  (run via `tsx`; PAPI `Revive.instantiate_with_code`, reading bin from `Photos.json`). It can
  share the app's own TS
  deploy/encode helpers with the UI. A `just photos deploy` recipe just invokes it, then injects
  the resulting address (reusing the landing-page injection mechanism, `landing/inject-config.mjs`).
- **Fallback**: a dev-only "Deploy contract" affordance in the UI when no address is configured
  (deploys the same `Photos.json` bin directly from the browser via `Revive.instantiate_with_code`).

## Integration points

- **Landing page** (`user-interfaces/landing/index.html`): add a `<a class="card" data-app="photos" …>` card and a `'photos': './photos/'` entry in `BASES`.
- **Workspace**: add `photos` to `user-interfaces/pnpm-workspace.yaml` and the `run-local-uis` skill.
- **CI**: add to the build matrix in `ui-checks.yml` and build+assemble steps in `deploy-ui.yml` (`dist → _site/photos`, `404.html`).
- **Descriptors**: reuse the `Revive`-inclusive PAPI descriptors (as `examples/papi` uses).

## Testing

- **Integration** (the headless source of truth, mirroring [`sc-team-drive.ts`](../../examples/papi/sc-team-drive.ts)):
  deploy `Photos` → `createLibrary(chosenProvider)` → `mkdir` an album → `PUT` photo + thumbnail →
  recompute the root locally → `setRoot` → re-list and assert the locally-recomputed root equals
  the on-chain anchor → `PUT` an edited photo (COW) → `setRoot` → assert library state, ownership, and the
  Writer grant. Add as a **TypeScript** flow in the app — `scripts/photos-flow.ts` (run via `tsx`)
  — reusing the same app-local TS helpers (`Photos.json` ABI, negotiate, `FileSystemClient` FS ops) the
  UI uses, plus a `just photos flow` recipe.
- **UI e2e** (Playwright + `@web3-storage/test-helpers`): the two states + create album + upload +
  edit + download.
- **Contract**: covered by the integration script; optional Solidity unit tests if a harness is
  added.

## Implementation milestones

Built as **minimal, independently reviewable milestones**. The strategy is to prove the entire
backend headless first (contract → bucket → albums → editing), because that's where the risk lives
(precompile origin, `msg.value`→payment, account mapping, `setMember` → provider write auth, the root
anchor); only then build UI on a foundation that already works. All contract source, build, and
deploy/flow scripts are **TypeScript and live in the app**.

### M1 — `Photos.sol` + deploy + `createLibrary` (headless)

The riskiest seam, isolated. Vendor `contracts/{Photos.sol,IWeb3Storage.sol}` in the app;
compile via `resolc` to `src/contract/Photos.json`. TS deploy script `scripts/deploy-contract.ts`
+ `just photos deploy`. Headless: `ensureAccountMapped` → deploy → `negotiate` terms (owner = the
contract's mapped account) → `createLibrary(userAccount, provider, terms, signature){value}`
→ read back `libraryOf` unsigned (`ReviveApi.call` + viem `decodeFunctionResult`).
**Done:** bucket exists with the contract account as admin, the chosen provider's agreement is active,
the user holds a Writer role on the bucket, `libraryOf.exists`; payment math verified against the
provider's signed `price_per_byte` (`NativeToEthRatio = 10^6`).

### M2 — Albums + blobs + thumbnails + root anchor (headless)

Use the SDK's `FileSystemClient`:
`mkdir` an album → `PUT` a real multi-MB photo + a placeholder thumbnail blob (real canvas
downscaling is browser-only; it lands in M6). Implement the **client-side root**: the deterministic
blake2-256 Merkle over sorted `(path, data_root, size)` entries (`metadataMerkleRoot` in
`packages/core/src/merkle.ts`) → `setRoot(rootCid)`. Verify: re-`ls`, byte-compare a downloaded
photo against its `data_root`, recompute the root locally and assert it equals the on-chain anchor;
a tampered tree fails the local recompute. **Done:** round-trip a photo through an album with a client-computed on-chain anchor proven.

### M3 — Full headless flow → CI source of truth

Complete `scripts/photos-flow.ts` + `just photos flow` (mirrors `just sc-team-drive`): create →
album → upload → **edit (COW)** → `setRoot` → download → assert library state, bucket admin,
Writer grant, and anchor. **Done:** one command runs deploy → create → albums → upload → edit →
assert against a local chain+provider. The entire backend is now proven with zero UI.

### M4 — UI skeleton (state detection only)

Scaffold `user-interfaces/photos/` mirroring `provider/` (React 19 + Vite + Tailwind + PAPI;
dev-accounts + extension wallet; `viem` dep; base `/web3-storage/photos/`; port **5178**).
Plumbing: add to `pnpm-workspace.yaml`, `run-local-uis`, landing card + `BASES`, `ui-checks.yml`
matrix, `deploy-ui.yml` assemble; add `photosContract?: H160` to `NetworkConfig`. App reads
`libraryOf` unsigned and renders **State A vs State B**. **Done:** runs locally on 5178, connects
a dev account, shows "no library" vs "Bucket #N". No writes.

### M5 — State A in UI: create library

Provider list from `StorageProvider.Providers` (price/capacity/accepting); size/duration inputs;
payment compute + buffer with `value` in **substrate atomic units** (labeled in tokens);
idempotent `Revive.map_account()` before first write; negotiate terms then `createLibrary` via
`Revive.call` `createAndSubmit`; transition to State B. Surfaces `PaymentExceedsMax` and
negotiate/expired-terms errors clearly. **Done:** a fresh account goes A→B in the browser.

### M6 — State B in UI: albums + upload + grid + view

Port the M2 FS layer to the browser. Albums: list/create folders. Upload: generate a downscaled
thumbnail (canvas → JPEG, longest edge ~320px), `PUT` the full photo + thumb, then recompute the
root locally → `setRoot`. Grid: `ls` an album, render from thumbnails (kilobytes per cell); open a
photo in a lightbox via `downloadFile`. **Done:** create albums, upload several photos,
reload, grid renders from thumbnails, opening one downloads full-res.

### M7 — Image editing

In-browser crop/rotate (canvas); `PUT` the edited result (replace the path or save as a copy);
recompute the root locally → `setRoot`. Show that the original lingers (copy-on-write, no GC). **Done:**
edit a photo, see the edit persist and reload, with the on-chain anchor updated.

### M8 — Tests + polish

Playwright e2e (two states + create album + upload + edit + download) via
`@web3-storage/test-helpers`; dev-only "Deploy contract" fallback when `photosContract` is unset;
final error/edge pass (unmapped account, negotiate/expired terms, provider write auth, integrity mismatch,
upload retry). Optional Solidity unit tests if a harness is added.

## Open questions / follow-ups

- **Nested sub-albums** — deeper directory nesting (the same model, more levels).
- **Multi-provider redundancy** — protocol **replicas** (`add_replica_provider`) for
  durability; native-only today, a future precompile/contract extension.
- **Client-side encryption** — drive-ui already has a `crypto.ts`; encrypt blobs before `PUT` so
  the provider holds only ciphertext.
- **Per-user refunds** of unused agreement reserve (v1 leaves it in the contract).
