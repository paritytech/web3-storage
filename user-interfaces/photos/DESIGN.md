# Photos — decentralized photo storage dApp (prototype)

## Goal

Photos is a normal photo app backed by Web3 Storage. A custom Solidity contract
(`Photos.sol`) drives **Layer 1** (the drive registry) through the drive-registry precompile,
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
| Storage layer | **Layer 1 (drive registry)** | Directories give us **albums for free**, and the provider's `/fs` API + drive-ui's client are reusable. |
| Contract calls | **PAPI `Revive` dispatchables** (`call`, `instantiate_with_code`), **viem for ABI only** | The CI-verified [`sc-api.js`](../../examples/papi/sc-api.js) / [`sc-team-drive.js`](../../examples/papi/sc-team-drive.js) pattern. |
| EVM JSON-RPC / MetaMask | **Out of scope** | Runtime is eth-rpc-ready (`runtimes/web3-storage-local/src/revive.rs`), so a MetaMask UX is a clean future follow-up. |
| Architecture | **Layer 1 + custom `Photos` contract, contract-owned drive per user** | The contract creates/owns the drive via the **drive-registry precompile** (`0x…0902`) and anchors the album-tree root on-chain — a real job the bare registry doesn't do. |
| Provider model | **Single user-chosen primary provider** per drive | Post-#97 `create_drive` opens a bucket + one primary atomically; the user picks the provider at creation. |
| Album/tree state | **Off-chain directory tree on the provider + on-chain root anchor in the contract** | The `/fs` API holds the tree; the contract stores a **client-computed** `metadata_merkle_root` as an integrity anchor. |
| Library structure (v1) | **Albums = directories** (one level of folders) | Nested sub-albums are a later extension of the same directory model. |

### Why Layer 1 (and where the contract fits)

The drive registry gives a real **directory tree** per drive (the provider's `/fs` API), so
albums and folders come for free instead of being hand-rolled into a manifest. Post-#97,
`create_drive` takes an explicit `(provider, terms, signature)` — the user still **chooses** the
provider — so Layer 1 no longer abstracts away provider choice.

The custom contract is still the headline integration: the **drive-registry precompile**
(`IDriveRegistry`, `0x…09020000`) lets `Photos.sol` create and own a drive on the user's behalf
and grant the user write access — exactly the pattern proven by
[`SharedTeamDrive.sol`](../../examples/papi/sc-team-drive.js). On top of that, the contract stores
each drive's current album-tree root CID on-chain (`setRoot`), which the drive registry itself
does not track — giving the contract a genuine job and the app a demonstrable integrity property.

## Architecture

The contract is the per-user **control plane** (drive lifecycle + the on-chain root anchor).
Photo blobs and the album/directory tree live off-chain on the chosen provider,
content-addressed by blake2-256.

```
Photos UI (React · dev-account/extension wallet · viem for ABI)
   │  PAPI: Revive.call (writes) · ReviveApi.call (unsigned reads)
   │  HTTP: /fs/{bucketId}/… (albums, photos, thumbnails) — direct, bypasses the contract
   ▼
Photos.sol (PolkaVM)            per user: { driveId, rootCid }
   │  CALL 0x…09020000 (drive-registry precompile)
   ▼
pallet_drive_registry           drive owned by the contract account · user granted Writer
   │  (folds in Layer 0: bucket + one primary via establish_storage_agreement_internal)
   ▼
provider node  /fs/{bucketId}/…    holds the photo blobs, thumbnails, and the directory tree
        (off-chain, browser ↔ provider; client-computed tree root anchored on-chain by the contract)
```

Origin model (from [`smart-contracts.md`](../../docs/design/smart-contracts.md)): precompile calls dispatch as
`RawOrigin::Signed(contract_account)`, so the **contract** owns every user's drive. Per-user
attribution lives in the contract (`driveOwner`). At creation the contract grants the user a
**Writer** role on the drive (`shareDrive` → `set_member_internal` on the storage-provider
pallet), so the browser can perform off-chain `/fs` operations directly with the user's own
wallet. This is custodial-by-ownership only — the transparent contract enforces "only you manage
your library."

## The `Photos` contract

The contract is **part of the app**, not a shared example: its source lives at
`contracts/Photos.sol`, with the `IDriveRegistry.sol` interface it imports vendored alongside it so
the app is self-contained. It compiles (via `resolc`, like `examples/contracts/build.sh`) to an
ABI + bytecode artifact at `src/contract/Photos.json`, which both the headless deploy recipe and
the UI import directly (the UI needs the ABI for viem encode/decode anyway).

Calls only the drive-registry precompile (`IDriveRegistry`, `0x…09020000`).

```solidity
contract Photos {
    IDriveRegistry constant DRIVES = IDriveRegistry(0x0000000000000000000000000000000009020000);

    uint8 constant ROLE_WRITER = 1; // 0 = Admin, 1 = Writer, 2 = Reader

    struct Library { uint64 driveId; bytes32 rootCid; bool exists; }
    mapping(address => Library) public libraries;    // user → their library
    mapping(uint64 => address)  public driveOwner;   // ownership guard

    event LibraryCreated(address indexed user, uint64 indexed driveId, bytes32 provider);
    event RootUpdated   (address indexed user, uint64 indexed driveId, bytes32 rootCid);

    /// Create my library with a provider I chose. `msg.value` funds the agreement payment,
    /// reserved from the contract's balance when the precompile dispatches. The contract owns
    /// the drive and grants me (`userAccount`, my substrate AccountId32) a Writer role so my
    /// browser can upload/list directly against the provider's `/fs` API.
    function createLibrary(
        bytes32 userAccount,
        string calldata name,
        bytes32 provider,
        IDriveRegistry.PrimitiveAgreementTerms calldata terms,
        bytes calldata signature
    ) external payable returns (uint64 driveId) {
        require(!libraries[msg.sender].exists, "library exists");
        driveId = DRIVES.createDrive(name, provider, terms, signature);
        DRIVES.shareDrive(driveId, userAccount, ROLE_WRITER);
        libraries[msg.sender] = Library(driveId, bytes32(0), true);
        driveOwner[driveId] = msg.sender;
        emit LibraryCreated(msg.sender, driveId, provider);
    }

    /// Anchor the current album-tree root on-chain after the client mutated the tree off-chain
    /// (upload / new album / edit / delete). `rootCid` is the metadata Merkle root the client
    /// computes itself over the drive's sorted (path, data_root, size) entries.
    function setRoot(bytes32 rootCid) external {
        Library storage lib = libraries[msg.sender];
        require(lib.exists, "no library");
        lib.rootCid = rootCid;
        emit RootUpdated(msg.sender, lib.driveId, rootCid);
    }

    /// UI reads this unsigned via `ReviveApi.call` (no signature, no gas) for state detection
    /// and to fetch the integrity anchor.
    function libraryOf(address user)
        external view returns (uint64 driveId, bytes32 rootCid, bool exists)
    {
        Library memory l = libraries[user];
        return (l.driveId, l.rootCid, l.exists);
    }
}
```

Notes:
- `bytes32 provider` / `bytes32 userAccount` are substrate `AccountId32`s (raw 32-byte sr25519
  pubkeys), per the precompile's type-encoding rules. `userAccount` is the signed-in user's own
  substrate account — the one their wallet signs `/fs` requests with.
- `terms` is the precompile's `PrimitiveAgreementTerms`; `terms.owner` must be the contract's
  substrate-mapped account (the drive owner). For a primary agreement, `hasBucketId = false` and
  `hasReplicaParams = false`. `price_per_byte` comes from the provider's **signed** terms.
- Precompile selectors used: `createDrive`, `shareDrive` (and `deleteDrive` for an optional
  `deleteLibrary`). `setRoot`/`libraryOf` are the contract's own state — the on-chain anchor that
  Layer 1 doesn't provide. **No precompile changes needed.**
- The contract is the drive admin (it created the drive), so the drive-registry / `ensure_admin`
  checks pass.

### Payment

`payment = price_per_byte × max_bytes × duration`, where `price_per_byte` is the value the
provider locked into the **signed terms** (from its `/negotiate` response). `msg.value` (eth-side)
funds the contract's substrate-mapped account; `pallet_revive` converts at `NativeToEthRatio =
10^6`. `create_drive` then reserves the payment from the contract's balance via Layer 0. The UI
sets `msg.value` from the computed payment plus a buffer. Unused reserve stays in the contract in
v1 (per-user refunds = a follow-up; acceptable for a prototype).

## Albums, blobs & the root anchor

Layer 1 gives a real directory tree per drive, served by the provider's path-based `/fs` API:

- `POST /fs/{bucketId}/mkdir` — create an album (a directory, e.g. `/Vacation`).
- `GET  /fs/{bucketId}/ls?path=…` — list an album's entries (files + sub-directories).
- `PUT  /fs/{bucketId}/file?path=…` / `GET …/file?path=…` — write / read a photo blob (the
  client wraps the existing chunking; multi-MB photos stream in chunks).
- `GET  /fs/{bucketId}/index_root` — the provider's view of the drive's `metadata_merkle_root`
  (a convenience cross-check only — the anchored value is always client-computed; see below).

The drive-ui client (`user-interfaces/drive-ui/src/lib/drive-client.ts`:
`listDirectory`/`uploadFile`/`downloadFile`, plus `mkdir`) already wraps these — reuse it rather
than reinventing.

- **Albums**: directories. v1 ships one level of folders (`/Beach`, `/Family`); the same model
  nests for sub-albums later.
- **Thumbnails**: each photo gets a small, downscaled JPEG (longest edge ~320px) generated
  client-side at upload time and stored as its own file under a parallel `.thumbs/` subtree
  (e.g. photo `/Beach/x.jpg` → thumb `/.thumbs/Beach/x.jpg`). The grid renders from thumbnails so
  listing an album downloads kilobytes per photo, not megabytes; the full file is fetched only
  when a photo is opened.
- **Integrity anchor (client-computed)**: the drive's metadata root is a *deterministic*
  blake2-256 Merkle tree over the drive's **sorted** `(path, data_root, size)` entries
  (`crates/providers/storage/src/index/fs.rs`), where each file's `data_root` is the content root the client
  already produces while chunking the upload. The client therefore **computes the root itself**
  rather than trusting the provider. After any mutation it anchors the locally-computed root via
  `setRoot(rootCid)`. To verify a library it recomputes the root from a fresh `ls` plus the
  downloaded files (checking each file against its own `data_root`) and asserts it equals the
  on-chain `rootCid` from `libraryOf` — a provider that hides, adds, swaps, or tampers with any
  file produces a mismatch. Anchoring the provider's `index_root` instead would be circular (it
  compares the provider's claim against the provider's claim); `/fs/index_root` is only a cheap
  cross-check. Thumbnails are stored as ordinary files, so they're covered by the same root.

## Data mutability & editing

Storage is **copy-on-write**: blobs are immutable (content-addressed by blake2-256, committed to
an append-only MMR — `crates/providers/storage/src/backend/disk.rs`). You never edit bytes in place; a
`PUT` to a path writes a **new** blob (new CID) and repoints that path in the tree. The album
tree's root changes, so each mutation ends with a freshly **recomputed** root → `setRoot`.

**Client-side image editing** (crop, rotate, filters) fits the same model directly: edit in the
browser, `PUT` the result back (to the same path to replace, or a new path to keep both), then
recompute the root locally and `setRoot`. The pre-edit bytes linger as a superseded blob.

Implications:
- **No garbage collection.** Superseded blobs (pre-edit photos, replaced thumbnails) are never
  reclaimed; they persist for the agreement's life. FS deletes only drop the path→CID mapping.
- **Quota = total of all versions.** An agreement pays for `max_bytes × duration` up front;
  accumulated versions consume that quota. To grow it, top up the agreement
  (`additional_bytes × remaining_duration × price`). Budget for the sum of all versions.

## Provider model

- **Single chosen primary.** `create_drive` opens one Layer 0 bucket + one primary agreement
  atomically; the user picks the provider at library creation. (Redundancy via protocol replicas
  is a native-only follow-up — see open questions.)
- **No auto-accept polling.** The provider signs the deal terms off-chain at `POST /negotiate`
  (`provider-node/src/api.rs`); the client redeems that signature on-chain via the contract's
  `createLibrary` → `createDrive`. The signature is synchronous consent, so the drive is active
  as soon as the extrinsic is included — no waiting for the provider to accept.

## Data flows

**Create library (State A → B)**
1. UI lists providers from `StorageProvider.Providers` (price, capacity, accepting). User picks one.
2. Ensure `Revive.map_account()` for the user (once, idempotent).
3. `POST /negotiate` to the chosen provider for signed `terms` (owner = the contract's
   substrate-mapped account); shape them into `PrimitiveAgreementTerms` (reuse
   `negotiatePrecompileTerms`).
4. `createLibrary(userAccount, name, provider, terms, signature)` via `Revive.call` with
   `value` = payment + buffer. The drive is active on inclusion; → State B.

**Create an album**
- `POST /fs/{bucketId}/mkdir` for the new folder → recompute the tree root locally → `setRoot`.

**Upload a photo**
1. Generate a downscaled thumbnail in the browser (canvas → JPEG, longest edge ~320px).
2. `PUT /fs/{bucketId}/file?path=/Album/photo.jpg` (full) and `…?path=/.thumbs/Album/photo.jpg`
   (thumb), keeping each file's locally-computed `data_root`.
3. Recompute the drive's metadata Merkle root locally → `setRoot(rootCid)` — one cheap tx.

**Edit a photo**
- Crop/rotate in the browser → `PUT` the result (same path to replace, or a new path to keep
  both) → recompute the root locally → `setRoot`. Copy-on-write; the original lingers.

**List / view**
- List an album: `GET /fs/{bucketId}/ls?path=/Album`, render the grid from each entry's thumbnail
  (kilobytes per cell); recompute the tree root locally from the listing (+ downloaded files) and
  check it equals the on-chain anchor.
- View: open a photo → `GET /fs/{bucketId}/file?path=…` (full resolution) in a lightbox.

## Front-end app

This app matches the React 19 + Vite + Tailwind + PAPI stack and the shared packages
(`@web3-storage/{network-config,network-picker,papi}`), reusing drive-ui's `drive-client`/`crypto`
patterns.

| Concern | Choice |
| --- | --- |
| Dev port | **5178** (landing 5176, drive 5174, provider 5175, s3 5177) |
| Wallet | Dev accounts (zero-setup) **and** Polkadot extension, like the provider UI |
| New dep | `viem` (ABI encode/decode only) |
| Reads | `ReviveApi.call` dry-run + viem `decodeFunctionResult` (unsigned) |
| Writes | `Revive.call` / `Revive.instantiate_with_code` via PAPI `signSubmitAndWatch` |
| FS ops | provider `/fs/{bucketId}/…` via the reused drive-client |
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
│  STATE B — library (drive #N · provider alice ●)             │
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
- **`/fs` authorization** → with provider auth enabled, the browser signs `/fs` requests with the
  user's wallet; the Writer role granted at `createLibrary` (`shareDrive`) makes them pass. (Dev
  chains may run `/fs` auth disabled, in which case the grant is unnecessary but still correct.)
- **Integrity mismatch** → reject/flag a library whose locally-recomputed metadata root ≠ the
  on-chain `rootCid`.
- **Upload retry** → `PUT` is idempotent for the same bytes (content-addressed); `setRoot` is the
  last step, so a retried upload re-anchors the same root.

## Contract deployment

Deployed **once per network**; the UI never asks a user to deploy. Everything contract-related —
source, build, and deploy — lives **inside the app**, so Photos is self-contained:
- **Source & build**: `contracts/{Photos.sol,IDriveRegistry.sol}` compiled with `resolc` to
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

- **Integration** (the headless source of truth, mirroring [`sc-team-drive.js`](../../examples/papi/sc-team-drive.js)):
  deploy `Photos` → `createLibrary(chosenProvider)` → `mkdir` an album → `PUT` photo + thumbnail →
  recompute the root locally → `setRoot` → re-list and assert the locally-recomputed root equals
  the on-chain anchor → `PUT` an edited photo (COW) → `setRoot` → assert library state, ownership, and the
  Writer grant. Add as a **TypeScript** flow in the app — `scripts/photos-flow.ts` (run via `tsx`)
  — reusing the same app-local TS helpers (`Photos.json` ABI, negotiate, drive-client FS ops) the
  UI uses, plus a `just photos flow` recipe.
- **UI e2e** (Playwright + `@web3-storage/test-helpers`): the two states + create album + upload +
  edit + download.
- **Contract**: covered by the integration script; optional Solidity unit tests if a harness is
  added.

## Implementation milestones

Built as **minimal, independently reviewable milestones**. The strategy is to prove the entire
backend headless first (contract → drive → albums → editing), because that's where the risk lives
(precompile origin, `msg.value`→payment, account mapping, `shareDrive` → `/fs` auth, the root
anchor); only then build UI on a foundation that already works. All contract source, build, and
deploy/flow scripts are **TypeScript and live in the app**.

### M1 — `Photos.sol` + deploy + `createLibrary` (headless)

The riskiest seam, isolated. Vendor `contracts/{Photos.sol,IDriveRegistry.sol}` in the app;
compile via `resolc` to `src/contract/Photos.json`. TS deploy script `scripts/deploy-contract.ts`
+ `just photos deploy`. Headless: `ensureAccountMapped` → deploy → `negotiate` terms (owner = the
contract's mapped account) → `createLibrary(userAccount, name, provider, terms, signature){value}`
→ read back `libraryOf` unsigned (`ReviveApi.call` + viem `decodeFunctionResult`).
**Done:** drive exists, owned by the contract account, the chosen provider's agreement is active,
the user holds a Writer role on the bucket, `libraryOf.exists`; payment math verified against the
provider's signed `price_per_byte` (`NativeToEthRatio = 10^6`).

### M2 — Albums + blobs + thumbnails + root anchor (headless)

Drive the provider's `/fs` API (reuse drive-ui's `drive-client` chunking, ported to app-local TS):
`mkdir` an album → `PUT` a real multi-MB photo + a placeholder thumbnail blob (real canvas
downscaling is browser-only; it lands in M6). Implement the **client-side root**: the deterministic
blake2-256 Merkle over sorted `(path, data_root, size)` entries (mirroring
`crates/providers/storage/src/index/fs.rs`) → `setRoot(rootCid)`. Verify: re-`ls`, byte-compare a downloaded
photo against its `data_root`, recompute the root locally and assert it equals the on-chain anchor
(and, as a sanity cross-check, the provider's `index_root`); a tampered tree fails the local
recompute. **Done:** round-trip a photo through an album with a client-computed on-chain anchor proven.

### M3 — Full headless flow → CI source of truth

Complete `scripts/photos-flow.ts` + `just photos flow` (mirrors `just sc-team-drive`): create →
album → upload → **edit (COW)** → `setRoot` → download → assert library state, drive ownership,
Writer grant, and anchor. **Done:** one command runs deploy → create → albums → upload → edit →
assert against a local chain+provider. The entire backend is now proven with zero UI.

### M4 — UI skeleton (state detection only)

Scaffold `user-interfaces/photos/` mirroring `provider/` (React 19 + Vite + Tailwind + PAPI;
dev-accounts + extension wallet; `viem` dep; base `/web3-storage/photos/`; port **5178**).
Plumbing: add to `pnpm-workspace.yaml`, `run-local-uis`, landing card + `BASES`, `ui-checks.yml`
matrix, `deploy-ui.yml` assemble; add `photosContract?: H160` to `NetworkConfig`. App reads
`libraryOf` unsigned and renders **State A vs State B**. **Done:** runs locally on 5178, connects
a dev account, shows "no library" vs "drive #N". No writes.

### M5 — State A in UI: create library

Provider list from `StorageProvider.Providers` (price/capacity/accepting); size/duration inputs;
payment compute + buffer with `value` in **substrate atomic units** (labeled in tokens);
idempotent `Revive.map_account()` before first write; negotiate terms then `createLibrary` via
`Revive.call` `signSubmitAndWatch`; transition to State B. Surfaces `PaymentExceedsMax` and
negotiate/expired-terms errors clearly. **Done:** a fresh account goes A→B in the browser.

### M6 — State B in UI: albums + upload + grid + view

Port the M2 FS layer to the browser. Albums: list/create folders. Upload: generate a downscaled
thumbnail (canvas → JPEG, longest edge ~320px), `PUT` the full photo + thumb, then recompute the
root locally → `setRoot`. Grid: `ls` an album, render from thumbnails (kilobytes per cell); open a
photo in a lightbox via the full `file?path=`. **Done:** create albums, upload several photos,
reload, grid renders from thumbnails, opening one downloads full-res.

### M7 — Image editing

In-browser crop/rotate (canvas); `PUT` the edited result (replace the path or save as a copy);
recompute the root locally → `setRoot`. Show that the original lingers (copy-on-write, no GC). **Done:**
edit a photo, see the edit persist and reload, with the on-chain anchor updated.

### M8 — Tests + polish

Playwright e2e (two states + create album + upload + edit + download) via
`@web3-storage/test-helpers`; dev-only "Deploy contract" fallback when `photosContract` is unset;
final error/edge pass (unmapped account, negotiate/expired terms, `/fs` auth, integrity mismatch,
upload retry). Optional Solidity unit tests if a harness is added.

## Open questions / follow-ups

- **Nested sub-albums** — deeper directory nesting (the same model, more levels).
- **Multi-provider redundancy** — protocol **replicas** (`establish_replica_agreement`) for
  durability; native-only today, a future precompile/contract extension.
- **Client-side encryption** — drive-ui already has a `crypto.ts`; encrypt blobs before `PUT` so
  the provider holds only ciphertext.
- **Library deletion** (`deleteLibrary` → `deleteDrive`) ending the agreement and refunding the
  remaining payment.
- **Per-user refunds** of unused agreement reserve (v1 leaves it in the contract).
