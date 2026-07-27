# Challenge-protocol Quint spec

Tracks [#265](https://github.com/paritytech/web3-storage/issues/265). No issues have been filed for the findings yet.

## What is here

| File | Contents |
| --- | --- |
| `challenges.qnt` | The model: state, actions, invariants, witnesses. Ends with two instance modules, `challengesCode` and `challengesDesign`. |
| `challenges_test.qnt` | 10 deterministic scenarios in two modules, `challengesCodeTest` and `challengesDesignTest`. |

The model covers the challenge lifecycle for one bucket: a primary (`P1`), a replica (`P2`), an authorized challenger (`Adm`, who is both bucket admin and agreement owner), a public challenger (`Pub`), and the Treasury (`Tre`). Actions mirror the pallet one-for-one — the three entry points (`challenge_checkpoint` / `challenge_offchain` / `challenge_replica`), `respond_to_challenge` with all three response variants, the `on_initialize` timeout sweep, checkpointing, replica sync, and the two-phase deregistration. Crypto is a ghost: `leaves` is the set of sequence numbers a provider physically holds, and a proof or signature verifies exactly when that ghost state supports it. MMR and Merkle internals are out of scope.

`STRICT_PROOF` is the switch that makes the two instances differ. `challengesCode` (`false`) models proof verification **as implemented** — `verify_mmr_proof(proof, &challenge.mmr_root)` never binds the proof to `challenge.target.leaf_index`. `challengesDesign` (`true`) models it **as specified** — `docs/design/scalable-web3-storage-implementation.md` passes `challenge.leaf_index` into `verify_mmr_proof`. Running both is the point: the difference between the two result columns is the divergence report.

Source correspondence, for keeping the spec honest when the code moves:

```
challenges.qnt  ↔  crates/pallets/storage-provider/src/impls/challenges.rs   (create / sweep / slash)
                ↔  crates/pallets/storage-provider/src/lib.rs                (extrinsics, on_initialize, deregister)
                ↔  crates/primitives/storage/src/lib.rs                      (Commitment ranges, proof verification)
Invariants      ↔  docs/design/scalable-web3-storage.md §"The Challenge Game"
                ↔  docs/design/scalable-web3-storage-implementation.md §"Challenge Protocol"
```

## How to run it

Requires the `quint` CLI (verified against 0.32.0). Everything below runs from the repo root and takes well under a second each.

```bash
quint typecheck specs/quint/challenges.qnt

# all invariants at once, per instance
quint run specs/quint/challenges.qnt --main challengesCode   --invariant allInvariants --max-steps 15 --max-samples 3000
quint run specs/quint/challenges.qnt --main challengesDesign --invariant allInvariants --max-steps 15 --max-samples 3000

# a single invariant, when attributing a violation
quint run specs/quint/challenges.qnt --main challengesCode --invariant defendedImpliesEntitled --max-steps 15 --max-samples 3000 --verbosity 3

# action coverage — every witness must report > 0 traces
quint run specs/quint/challenges.qnt --main challengesCode --max-steps 15 --max-samples 2000 \
  --witnesses witnessDefended witnessTimeoutSlash witnessInvalidSlash witnessDeletedDefense \
              witnessSupersededDefense witnessDeregistered witnessOutOfRangeChallenge witnessCapFull

# scenarios — --main is required, the test file has no module matching its stem
quint test specs/quint/challenges_test.qnt --main challengesCodeTest     # 8 passing
quint test specs/quint/challenges_test.qnt --main challengesDesignTest   # 2 passing
```

Use `quint run` throughout. `quint verify` (Apalache, exhaustive) has not been run against this spec and is not part of the workflow — reach for it only on an explicit request to model-check.

## Results

`allInvariants` fails on both instances. Attributed per invariant, at `--max-steps 15 --max-samples 3000`:

| Invariant | Issue #265 name | `challengesCode` | `challengesDesign` |
| --- | --- | --- | --- |
| `moneyConserved` | part of `depositConservation` | holds | holds |
| `reservedAccounted` | part of `depositConservation` | holds | holds |
| `resolvesOnce` | `challengeResolvesOnce` | holds | holds |
| `noExitWithLiability` | extra (exit safety) | holds | holds |
| `defendedImpliesEntitled` | `dishonestAlwaysSlashable` | **violated** | **violated** |
| `stakeIntactOnDefense` | extra (stake safety) | **violated** | **violated** |
| `honestNeverSlashed` | `honestNeverSlashed` | holds (see finding 4) | **violated** |

The four that hold were additionally re-run at `--max-steps 30 --max-samples 10000` with no violation. All eight witnesses report non-zero traces, so no action in `step` is dead; `witnessSupersededDefense` is the rarest at 0.75%.

## Findings

Each carries a deterministic scenario in `challenges_test.qnt`. Those scenarios assert the current, broken behaviour **on purpose** — they are green today and will fail the day a fix lands, which is the signal that the spec needs updating alongside it.

**1. Unbound MMR proof — any held leaf defends any challenge.** `verify_mmr_proof` (`crates/pallets/storage-provider/src/lib.rs:2356`) checks that the submitted leaf belongs to the challenged root but never that it sits at `challenge.target.leaf_index`. A provider that lost the challenged leaf but still holds any other leaf under the same root defends successfully and keeps its stake. Violates `defendedImpliesEntitled`; the design pseudocode does not have this hole. Scenario: `anyLeafProofEscapeTest`.

**2. `Superseded` is a permanent escape for a stale replica.** The rule at `lib.rs:2416` requires only that the challenged root differs from canonical and that the challenged sequence falls inside the canonical range — never that the provider holds anything. A replica whose `last_sync` lags canonical can drop all its data and defend every challenge on the canonical range, repeatably, until it re-syncs. Violates `defendedImpliesEntitled` on **both** instances, so fixing finding 1 does not close it. This one is faithful to the design text (`-implementation.md:2337` explicitly permits `challenged_seq < canonical_end`), which makes it a design finding, not an implementation bug. Scenarios: `supersededEscapeTest`, `supersededEscapeStillTest`.

**3. No challenger tier — every valid defense slashes provider stake.** The design (v2.2/2.3) states that a valid response never touches stake, and that public challengers pay 100% so the provider bears nothing. The `Challenge` struct (`lib.rs:647`) has no `authorized` field, though the design's own data-structures section specifies one (`-implementation.md:588`), and `respond_to_challenge` applies the authorized cost-split to everyone — slashing `provider_cost` from stake into the Treasury even on a correct public defense. Repeatable: a crowd of strangers grinds an honest provider's stake down one deposit-fraction at a time, which is the anti-DDoS drain the tier split exists to prevent. Violates `stakeIntactOnDefense`. Scenario: `defenseGrindsStakeTest`.

**4. Missing `leaf_index` bounds check.** No extrinsic validates `target.leaf_index < leaf_count`, so a challenge against a nonexistent leaf is accepted on-chain. Under design semantics an honest provider holding every byte has no valid defense to it — no proof exists, `Superseded` and `Deleted` do not apply — and is full-stake slashed by any stranger for the price of one deposit. Under code semantics this is masked by finding 1, since the stale-leaf proof "defends" it. Two bugs cancelling, which is exactly why `honestNeverSlashed` holds on the code instance and fails on the design instance. Scenarios: `outOfRangeGriefingTest` (design), `outOfRangeMaskedTest` (code).

Two more by inspection, not invariant violations and not modeled: there is **no `cancel_challenge` extrinsic** although the design specifies challenger cancellation before a response; and the maintainer's comment on #265 argues that only `Timeout` should slash, with invalid defenses leaving the challenge open instead — the spec currently models the code's slash-on-invalid, so it is the natural place to explore that change before writing any code.

## Decisions taken, with reasons

**No Choreo.** Choreo is scaffolding for N-process message-passing protocols — per-process local state, a message buffer, the listen/act `cue` split. This protocol is a single sequential state machine: the chain executing totally-ordered extrinsics over shared storage. No messages are exchanged and no per-process view diverges, so Choreo would add ceremony and no interleaving coverage. It would also hurt the MBT work below, since quint-connect needs `Config { state: &[...], nondet: &[...] }` path overrides to reach state nested inside a Choreo environment, whereas flat `var`s need none.

**No `basicSpells`.** It does not resolve against the installed CLI (`QNT013: could not load`) and would have to be vendored into this directory. It would save roughly ten lines — `Option` in place of the `-1` and `NO_COMMIT` sentinels, `min` for `imin`, `mapRemove` for `mapWithout` — in exchange for a ~100-line vendored file. The sentinels are also the better choice for MBT: every Quint `Option` forces an `#[serde(with = "As::<de::Option<_>>")]` annotation in the Rust state struct, while sentinel integers deserialize plainly.

**Instance sizing.** Two providers, `CHALLENGE_TIMEOUT = 3`, `MAX_PER_DEADLINE = 2`, four sequence numbers, three commitments. Issue #265 calls small instances sufficient and that holds — every finding here is logical, and each reproduces in a trace of eight steps or fewer.

## Remaining work from the issue

**CI job** (#265 TODO 3) is not written. It needs Node plus `npm i -g @informalsystems/quint` before running the typecheck, both `--main` invariant runs, the witness run, and both `quint test` invocations. Versions come from `.github/env`, never hardcoded. Note the job cannot be green until the findings are triaged, since `allInvariants` currently fails by design — either gate on the four holding invariants and the ten scenarios for now, or hold the workflow until the findings are resolved. Do not add `continue-on-error`.

**Issues per finding** (#265 TODO 4) are not filed. Findings 2 and 3 touch `docs/design/`, which is CODEOWNERS-gated, so they want a design-owner conversation rather than a quiet code patch.

**MBT via quint-connect** is researched but not built. The crate is [`quint-connect`](https://github.com/informalsystems/quint-connect) 0.1.2 (Informal Systems, Apache-2.0, last released May 2026), needing Rust ≥ 1.70 — we run 1.93 — and `quint` on `PATH`. It shells out to the CLI, generates `--mbt` traces, replays each step through a `Driver` trait, and after every step deserializes the spec state with the `itf` crate and compares it against `State::from_driver`, printing a diff on divergence.

The spec is already in the right shape for it, which I verified by running `quint run specs/quint/challenges.qnt --main challengesCode --mbt --n-traces=40 --max-steps 14 --max-samples 400 --out-itf=traces/out.itf.json`. `mbt::actionTaken` records the inner action names (`respondAs`, `challengeCheckpointAs`, `advanceBlockBy`, …) rather than `step`, and `mbt::nondetPicks` carries every pick by name including the nested ones — `k` as a tuple, `rsp` as a sum-type tag. Every action appears across 40 traces except `completeDeregP`, which needs roughly `--max-steps 25` to show up.

The build, in outline. A dev-only crate (`crates/pallets/storage-provider/mbt/`) with `quint-connect`, `itf` and `serde` declared in `[workspace.dependencies]`, carrying its own mock runtime whose Config constants match the spec instance exactly — `ChallengeTimeout = 3`, `DeregisterAnnouncementPeriod = 4`, `ChallengeDeposit = 10`, `MaxChallengesPerDeadline = 2`, stake 100, and `RequestTimeout = 1` to satisfy `integrity_test` — so no amount scaling is needed anywhere. The driver holds a `TestExternalities` plus an MMR fixture, which is the only real glue work: four deterministic chunks for sequences 0–3 built into genuine MMRs, so model roots 1, 2 and 3 map to real `H256` values in both directions. The `switch!` arms then map one-to-one onto the extrinsics, with `advanceBlockBy(jump)` bumping the mocked `BlockNumberProvider` and running `on_initialize` (which matches the spec's fused sweep), and `signOffchainC`, `adminDeleteTo` and `loseLeaf` handled as driver-side bookkeeping. For `respondAs` the driver submits the concrete response the model's adjudication assumed — for finding 1 that means submitting the other held leaf's real proof, which is what turns the model-level escape into a demonstration against the real pallet. `State::from_driver` reads `Providers`, `PendingChallenges`, `Challenges` (tuple-keyed, which `itf` handles), `NextChallengeIndex`, the bucket snapshot and balances, mirroring the ghost fields from driver bookkeeping. Tests are one `#[quint_run(spec = "...", main = "challengesCode", max_samples = 100, max_steps = 25)]` for random replay plus one `#[quint_test(..., main = "challengesCodeTest", test = "anyLeafProofEscapeTest")]` per finding as a regression; both macros accept `main`, so the multi-module layout works unchanged.

Worth stating plainly: because the spec models code semantics, MBT should **pass** against today's pallet. Its job is to lock the spec-to-code correspondence so that fixing the findings forces spec, driver and pallet to move together. The crate is 0.1.x, so expect API churn.

## Ground rules

The spec is the ground truth. Do not edit it to match broken code — if the two disagree, that is a finding to raise, and where the disagreement is with `docs/design/` it goes through a CODEOWNERS-reviewed PR. When the pallet's challenge logic changes, update the spec and re-run everything above before touching the implementation.

Out of scope in the current model, and deliberately so: payment streams and agreement settlement, multi-bucket and multi-agreement interactions, the sweep's per-block PoV budgets (`MAX_SWEEP_SPAN`, `MAX_SWEEP_SLASH_BUDGET`) and its one-block lag, which are collapsed into "advancing past the deadline sweeps it" — a simplification that only delays slashing, the safe direction for every invariant here — along with challenger transaction fees, signature replay and nonce recency, and challenge cancellation. Extending to funds conservation across agreements and checkpoints is listed as follow-up work in the issue itself.
