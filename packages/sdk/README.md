# @web3-storage/sdk

Umbrella SDK for the storage parachain. Consumers (UIs, test-helpers, the
PAPI examples/E2E suite) import this package; the physical layout behind it
implements #123's monorepo design:

```
packages/
  core/     @web3-storage/core     backend-free, browser-safe primitives:
                                   byte/hex utils, retrying httpFetch,
                                   provider request signing, CID verification
  layer0/   @web3-storage/layer0   the chain binding: typed PAPI wrappers per
                                   pallet, signers, tx submission, watchValue
                                   waits, provider-node HTTP, ./revive
  layer1/   @web3-storage/layer1   the storage interfaces: FileSystemClient
                                   (drives) and S3Client (buckets/objects),
                                   both on plain Layer 0 buckets
  sdk/      @web3-storage/sdk      this package: re-exports all three and
                                   hosts the Web3Storage facade
```

Dependency direction is strictly `layer1 → layer0 → core`; nothing points
back up.

## Entry points

- `@web3-storage/sdk` — everything: flat layer-0 functions, layer-1 clients,
  core primitives, and the facade:

  ```ts
  const w3s = await Web3Storage.connect("ws://127.0.0.1:2222", { signer: makeSigner("//Alice") });
  const { bucketId } = await w3s.fs.createDrive({ maxCapacity: 1n << 20n, storagePeriod: 100 });
  await w3s.fs.waitForProvider(bucketId);
  await w3s.fs.uploadFile(bucketId, "/hello.txt", new TextEncoder().encode("hi"));
  ```

- `@web3-storage/sdk/fs` / `@web3-storage/sdk/s3` — the layer-1 clients
  directly.
- `@web3-storage/sdk/revive` — `pallet_revive` helpers (deploy/call PolkaVM
  contracts). Separate so `viem` stays out of consumers that never touch
  contracts.

## Transaction semantics

Canonical rules, established by the E2E suite's finalization fixes
(`d3de2d9`, `391f8bf`/`de28f36`, `2bd19cf`, `18e1416`):

- **`submitTx` resolves at in-block inclusion (`mode: "best"`) by default** —
  ~6x faster than finalization, and read-your-writes holds as long as reads
  target the best block.
- **All reads after an in-block submit use `READ_OPTS` (`{at: "best"}`)** —
  in TESTS AND EXAMPLES. Real UIs keep the reorg-safe finalized view
  (PAPI's default, or `FINALIZED_READ_OPTS` explicitly), and the layer-1
  clients default to finalized reads + finalized submission; suites opt into
  test semantics via `readOpts: READ_OPTS, submitMode: "best"`.
- **`mode: "finalized"` is opt-in, for reorg-sensitive effects only.** A
  challenge id embeds its creation block, so the challenge-creating wrappers
  finalize internally; everything else stays in-block.
- **Events come from the tx result** (`requireOneEvent`), never from
  `api.event.X.watch()` — typed event watches observe only finalized blocks
  and race in-block submission. The wait helpers are built on
  `query.*.watchValue(..., {at: "best"})`, which replays the current value on
  subscribe and therefore has no missed-event window.

Every pallet wrapper accepts a trailing `SubmitOpts` so apps can override the
test-suite defaults: the layer-1 clients pass `retryStale: 0` (a user-visible
retry is the right UX, not an automatic one) and `onStatus: null` unless the
app supplies a listener. `submitTx` streams `created`/`in-pool`/`best`/
`finalized` phases with a `final` flag; the default console listener prints
only the final one.

## Buckets and their providers

Bucket creation and provider assignment are separate calls. A quote names the
bucket it is for, so negotiate it against the bucket the redeeming call
targets:

| Wrapper | Negotiate with | Redeems |
| --- | --- | --- |
| `createBucket` | — | `create_bucket` |
| `createBucketWithPrimary` | `bucket: null` | `create_bucket_with_primary` |
| `addPrimaryProvider` | `bucket: <id>` | `add_primary_provider` (admin only) |
| `addReplicaProvider` | `bucket: <id>` | `add_replica_provider` |

`signedTermsBucketId(signed)` returns the bucket id a signed quote names;
it returns `undefined` for a quote that creates its bucket.

### Changing a bucket's primary provider

The procedure is specified in the design doc, [Provider Lifecycle in
Bucket](../../docs/design/scalable-web3-storage-implementation.md#provider-lifecycle-in-bucket).
The wrappers for its steps are `addPrimaryProvider` (new provider joins),
`checkpoint` (new provider signs the snapshot after the client uploaded the
data to it) and `endAgreement` (old provider leaves).

## File systems and S3 buckets

`FileSystemClient` and `S3Client` keep their data in the Layer 0 bucket
itself and use only the provider's Layer 0 routes (`PUT /node`,
`POST /commit`, `GET /node`, `GET /read`, `GET /commitment`,
`GET /mmr_proof`). The format code exists in one module,
`packages/layer1/src/tree.ts`:

- Directories and files are SCALE-encoded `DirectoryNode` and
  `FileManifest` blobs (`crates/primitives/file-system`). A blob's CID is
  its `data_root` (`computeDataRoot`).
- The root directory's CID is the bucket's last MMR leaf. A bucket with no
  leaves is an empty drive.
- A write uploads the new content, its manifest and each rewritten
  directory up to the root, then commits them in one `/commit`, root last.
- An S3 key `k` is the file at path `/k`. Directories between are created
  on put and removed on delete when empty.

Rules for callers:

- **One writer at a time.** Two clients that write the same bucket
  concurrently can lose one change: the last commit wins.
- **Only these clients write the bucket.** A raw Layer 0 commit becomes the
  last leaf, and the tree no longer loads (`FileSystemError` with code
  `NotAFileSystem`).
- **An Admin prefix delete** (`/delete` with a new `start_seq`) can remove
  blobs that the current tree still references.
- **Reads are unauthenticated.** Anyone who knows a CID can read the blob
  (#383/#396). Encrypt confidential data on the client.
- **Times are in milliseconds.** `FileSystemClient` and `S3Client` return
  `mtime` and `lastModified` in milliseconds since the epoch. The stored
  format (`DirectoryEntry.mtime`) and the Rust clients use seconds, so the
  values are multiples of 1000.
- **`S3Client.listObjects` pages** contain at most 1000 entries (`maxKeys`
  is clamped to 1-1000), and each page reads the tree again. Use
  `listAllObjects` to get every key from one read.

## Download verification

| Path | Verified? | How |
| --- | --- | --- |
| `downloadChunk` | **Yes, throws `CidMismatchError`** | the requested hash is the node's hash |
| `readBlob` / `fs.downloadByCid` with a size | **Yes, throws `BlobVerificationError`** | every chunk is checked against the blob's padded Merkle tree |
| `readBlob` / `fs.downloadByCid` without a size | Partly | the tree is checked, but a provider can return an internal node as a 64-byte blob |
| `fs.downloadFile`, `s3.getObject` | **Yes** | the path resolves through CID-checked directory and manifest blobs from the root CID |

The root CID comes from the provider's last MMR leaf, checked against the
provider's MMR root with `verifyLastMmrLeaf`. That MMR root is the provider's
claim until it is compared with an on-chain checkpoint.

Provider requests are signed (`Web3Storage <pubkey>:<sig>:<timestamp>` per
the `crates/providers/auth` crate) whenever the signer carries a raw keypair
(`makeSigner` populates it). Wallet-extension signers can't produce the raw
sr25519 signature — unauthenticated providers still work. Client auth is
sr25519-only ([#304](https://github.com/paritytech/web3-storage/issues/304)
tracks extending it); *provider* signatures are multi-scheme and arrive as
SCALE-encoded `MultiSignature` hex, decoded by `decodeMultiSignature`.

## Deliberately NOT in this package

- **Test-only powers**: Sudo-based registry cleanup, `submitTxExpectFailure`,
  `ensureSoleAcceptingProvider`, Playwright fixtures —
  `@web3-storage/test-helpers` owns those. Nothing SDK-shaped should be able
  to `kill_storage`.
- **Demo/E2E orchestration**: CLI arg parsing, pretty-printing —
  `examples/papi/support.ts`.
- **UI state stores and status strings** — presentation. The waits expose an
  `onTick` callback so apps generate their own progress text.
- **Pre-subscription warmup guards** (`waitForChainReady`,
  `waitForBlockProduction`) stay polling-based on purpose: during zombienet
  warmup there are no blocks/metadata yet, so a subscription would hang.

## Canonical PAPI patterns (no `@polkadot/*`)

Repo rule (see the root `CLAUDE.md`): all JS/TS talks to the chain through
`polkadot-api` — never `@polkadot/keyring`, `@polkadot/util-crypto`,
`@polkadot/util`, `@polkadot/api`, or any other `@polkadot/*` package.
Consumers should import the helpers below from this SDK rather than
re-deriving them; the snippets document what the SDK does under the hood.

The signer/derive pattern behind `makeSigner` — set up the derive function
once at module load, then call `makeSigner("//Alice")` etc.:

```js
import { getTxCreator } from "polkadot-api/tx-creator";
import { sr25519CreateDerive } from "@polkadot-labs/hdkd";
import {
  DEV_PHRASE,
  entropyToMiniSecret,
  mnemonicToEntropy,
  ss58Address,
} from "@polkadot-labs/hdkd-helpers";

const devMiniSecret = entropyToMiniSecret(mnemonicToEntropy(DEV_PHRASE));
const deriveSr25519 = sr25519CreateDerive(devMiniSecret);

export function makeSigner(seed) {
  const keyPair = deriveSr25519(seed); // seed is a SURI path like "//Alice"
  return {
    signer: getTxCreator(keyPair.publicKey, "Sr25519", keyPair.sign),
    address: ss58Address(keyPair.publicKey), // prefix 42 (`5…`), same as @polkadot/keyring default
    publicKey: keyPair.publicKey,
    seed,
  };
}
```

No `cryptoWaitReady()` — hdkd is synchronous.

**SS58 gotcha**: `ss58Address` defaults to substrate prefix 42 (`5…`) while
PAPI surfaces accounts with the runtime SS58 prefix (Polkadot-style `1…` on
this parachain) — same key, different string, so string equality fails.
Compare raw bytes via `ss58Decode`:

```js
import { ss58Decode } from "@polkadot-labs/hdkd-helpers";

// ss58Decode(addr) → [bytes, prefix]
export function sameAddress(a, b) {
  try {
    const [aBytes] = ss58Decode(a);
    const [bBytes] = ss58Decode(b);
    if (aBytes.length !== bBytes.length) return false;
    for (let i = 0; i < aBytes.length; i++) {
      if (aBytes[i] !== bBytes[i]) return false;
    }
    return true;
  } catch {
    return false;
  }
}
```

## Descriptors

The typed API comes from the single tracked metadata snapshot owned by
`@web3-storage/papi` (`packages/papi`). CI re-fetches metadata
from the live chain in the E2E job and fails when the committed snapshot has
drifted from the runtime. To refresh locally (chain running on :2222):

```sh
pnpm run papi:generate
```
