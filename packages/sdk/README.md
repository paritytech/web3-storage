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
  storage-formats/ @web3-storage/storage-formats
                                   a file system and S3-style objects stored
                                   in a bucket: the on-bucket format, FsTree
                                   and S3Bucket; needs a provider URL and a
                                   signer, no chain
  sdk/      @web3-storage/sdk      this package: re-exports all three and
                                   hosts the Web3Storage class
```

Dependency direction is strictly `sdk → storage-formats → layer0 → core`;
nothing points back up.

## Entry points

- `@web3-storage/sdk` — everything: flat layer-0 functions, the file
  system, core primitives, and the `Web3Storage` class:

  ```ts
  const w3s = await Web3Storage.connect("ws://127.0.0.1:2222", { signer: makeSigner("//Alice") });
  const { bucketId } = await w3s.createBucket({ maxCapacity: 1n << 20n, duration: 100 });
  await w3s.waitForProvider(bucketId);
  const drive = await w3s.drive(bucketId);
  await drive.putFile("/hello.txt", new TextEncoder().encode("hi"));
  ```

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
  (PAPI's default, or `FINALIZED_READ_OPTS` explicitly), and `Web3Storage`
  defaults to finalized reads + finalized submission; suites opt into
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
test-suite defaults: `Web3Storage` passes `retryStale: 0` (a user-visible
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

`FsTree` (`Web3Storage.drive`) and `S3Bucket` (`Web3Storage.objects`) keep
their data in the bucket itself and use only the provider's blob routes.
The format, the tree and the S3 view are the Rust crate
`clients/storage-formats`, compiled to wasm;
`packages/storage-formats` loads it and implements its blob store over the
provider's HTTP API. `pnpm install` builds the wasm into
`packages/storage-formats/pkg` and needs rustup and `wasm-bindgen-cli` at the
version in `Cargo.lock`. Rebuild it after a change to the crate with
`pnpm --filter @web3-storage/storage-formats build:wasm`.

The module docs of the crate's `tree` module
(`cargo doc -p storage-formats --no-deps --open`) describe the format, the
root, the write order and the limits: one writer, no other writes to the
bucket, prefix deletes, blob reads by CID, bucket growth. Read them before
writing to a bucket.

Notes for callers:

- **Writes from one `Web3Storage`** to one bucket run one at a time. Other
  `Web3Storage` instances and other processes are not coordinated.
- **Times.** `FsTree` returns `mtime` in seconds, as stored
  (`DirectoryEntry.mtime`). `S3Bucket` returns `lastModified` in milliseconds
  since the epoch, as S3 does, so the values are multiples of 1000.
- **`S3Bucket.listObjects` pages** contain at most 1000 entries (`maxKeys`
  is clamped to 1-1000), and each page reads the tree again.
  `listAllObjects` returns every key.

## Download verification

| Path | Verified? | How |
| --- | --- | --- |
| `downloadChunk` | **Yes, throws `CidMismatchError`** | the requested hash is the node's hash |
| `readBlob` with a size | **Yes, throws `BlobVerificationError`** | every chunk is checked against the blob's Merkle tree |
| `readBlob` without a size | **Yes, throws `BlobVerificationError`** | every node is checked against its hash (a chunk as a leaf, an internal node over its children), and the chunks must rebuild the root |
| `FsTree.getFile`, `S3Bucket.getObject` | **Yes** | the path resolves through CID-checked directory and manifest blobs from the root CID |

The root CID comes from the provider's last MMR leaf, checked against the
provider's MMR root with `verifyMmrProof`. That MMR root is the provider's
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
