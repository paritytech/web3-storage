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
                                   (drives) and S3Client (buckets/objects)
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
  const { driveId, bucketId } = await w3s.fs.createDrive({ maxCapacity: 1n << 20n, storagePeriod: 100, payment: 10n ** 12n });
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
app supplies a listener. `submitTx` streams `signed`/`in-pool`/`best`/
`finalized` phases with a `final` flag; the default console listener prints
only the final one.

## Download verification

| Path | Verified? | Why |
| --- | --- | --- |
| `downloadChunk` / `fs.downloadByCid` | **Yes — throws `CidMismatchError`** | the requested hash IS the chunk's CID |
| `s3.getObject`, payload ≤ 256 KiB | **Yes — throws `CidMismatchError`** | single-chunk `data_root` equals the chunk hash; compared against the on-chain `S3Registry.Objects` cid |
| `s3.getObject`, payload > 256 KiB | `verified: false` flag | the on-chain cid is a Merkle root; reproducing it needs a DAG walk (Rust-client parity) — tracked separately |
| `fs.downloadFile` (by path) | No (documented) | the provider's `/fs` file route returns no `data_root` to check against |

Provider requests are signed (`Web3Storage <pubkey>:<sig>:<timestamp>` per
the `crates/providers/auth` crate) whenever the signer carries a raw keypair
(`makeSigner` populates it). Wallet-extension signers can't produce the raw
sr25519 signature — unauthenticated providers still work.

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

## Descriptors

The typed API comes from the single tracked metadata snapshot owned by
`@web3-storage/papi` (`packages/papi`). CI re-fetches metadata
from the live chain in the E2E job and fails when the committed snapshot has
drifted from the runtime. To refresh locally (chain running on :2222):

```sh
pnpm run papi:generate
```
