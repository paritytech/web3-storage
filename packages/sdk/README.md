# @web3-storage/sdk

Chain-interaction SDK for the storage parachain: typed PAPI extrinsic
wrappers, signer construction, transaction submission, provider-node HTTP
helpers, and `watchValue`-based chain waits. Consumed by the PAPI examples and
E2E suite (`examples/papi`), and — incrementally — by the UIs and
`@web3-storage/test-helpers`.

## Relationship to the dedicated TS SDK (issue #123)

This package is the **seed** of the dedicated TS SDK, not a competitor to it.
It deliberately ships flat typed functions (`createBucket(api, signer, opts)`)
rather than a class facade: every function maps 1:1 onto a pallet extrinsic or
a provider HTTP route, is tree-shakeable from Vite bundles, and can be
re-homed into #123's `core`/`layer0`/`layer1` package split without rewrites.
A future `Web3Storage.connect(ws)` / `Provider.connect(http)` facade is a thin
class capturing `{api, signer}` context and delegating here — additive work.

## Transaction semantics

Canonical rules, established by the E2E suite's finalization fixes
(`d3de2d9`, `391f8bf`/`de28f36`, `2bd19cf`, `18e1416`):

- **`submitTx` resolves at in-block inclusion (`mode: "best"`) by default** —
  ~6x faster than finalization, and read-your-writes holds as long as reads
  target the best block.
- **All reads after an in-block submit use `READ_OPTS` (`{at: "best"}`)** —
  the default finalized view lags inclusion and races just-written state.
- **`mode: "finalized"` is opt-in, for reorg-sensitive effects only.** A
  challenge id embeds its creation block, so the challenge-creating wrappers
  (`challengeOffchain`, `challengeCheckpoint`) finalize internally; everything
  else stays in-block.
- **Events come from the tx result** (`requireOneEvent`), never from
  `api.event.X.watch()` — typed event watches observe only finalized blocks
  and race in-block submission. For the same reason the wait helpers
  (`waitForAgreementAcceptance`, `waitForPrimaryProvider`, …) are built on
  `query.*.watchValue(..., {at: "best"})`, which replays the current value on
  subscribe and therefore has no missed-event window.

UIs override the test-suite defaults via `SubmitOpts`: pass `retryStale: 0`
(a user-visible retry is the right UX, not an automatic one) and `onStatus`
(progress callback; `null` silences the default console logging).

## Subpath exports

- `@web3-storage/sdk` — everything chain + provider-HTTP related.
- `@web3-storage/sdk/revive` — `pallet_revive` helpers (deploy/call PolkaVM
  contracts, decode contract events). Separate so `viem` stays out of the
  dependency graph of consumers that never touch contracts.

## Deliberately NOT in this package

- **Test-only powers**: Sudo-based registry cleanup, `submitTxExpectFailure`,
  Playwright fixtures — `@web3-storage/test-helpers` owns those. Nothing
  SDK-shaped should be able to `kill_storage`.
- **Demo/E2E orchestration**: CLI arg parsing, `ensureSoleAcceptingProvider`
  (signs for arbitrary dev keys to make provider auto-matching
  deterministic), pretty-printing — `examples/papi/support.js`.
- **The bespoke E2E runner** (`examples/papi/e2e/{runner,helpers,format}.js`)
  — single consumer, coupled to the workflow-file layout.
- **UI state stores and status strings** — presentation. The waits expose an
  `onTick` callback so apps generate their own progress text.
- **Pre-subscription warmup guards** (`waitForChainReady`,
  `waitForBlockProduction`) stay polling-based on purpose: during zombienet
  warmup there are no blocks/metadata yet, so a subscription would hang with
  nothing to react to.

## Descriptors

The typed API comes from the single tracked metadata snapshot owned by
`@web3-storage/papi` (`user-interfaces/shared/papi`). CI re-fetches metadata
from the live chain in the E2E job and fails when the committed snapshot has
drifted from the runtime. To refresh locally (chain running on :2222):

```sh
pnpm run papi:generate
```
