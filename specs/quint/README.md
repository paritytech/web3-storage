# Challenge-protocol Quint spec

Tracks [#265](https://github.com/paritytech/web3-storage/issues/265). Fixes for three of the four findings are open PRs, none merged — see [Finding status](#finding-status). The `challengesCode` instance and the finding scenarios model `dev`, not the open PRs.

## What is here

| File | Contents |
| --- | --- |
| `challenges.qnt` | The model: state, actions, invariants, witnesses. Ends with two instance modules, `challengesCode` and `challengesDesign`. |
| `challenges_test.qnt` | 10 deterministic scenarios in two modules, `challengesCodeTest` and `challengesDesignTest`. |
| `crates/pallets/storage-provider/mbt/` | Model-based testing crate: replays spec traces against the real pallet via [quint-connect](https://github.com/informalsystems/quint-connect). See [MBT](#mbt-via-quint-connect). |

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

**1. Unbound MMR proof — any held leaf defends any challenge.** `verify_mmr_proof` (`crates/primitives/storage/src/lib.rs`, called from `respond_to_challenge`) checks that the submitted leaf belongs to the challenged root but never that it sits at `challenge.target.leaf_index`. A provider that lost the challenged leaf but still holds any other leaf under the same root defends successfully and keeps its stake. Violates `defendedImpliesEntitled`; the design pseudocode does not have this hole. Scenario: `anyLeafProofEscapeTest`.

**2. `Superseded` is a permanent escape for a stale replica.** The `Superseded` arm of `respond_to_challenge` requires only that the challenged root differs from canonical and that the challenged sequence falls inside the canonical range — never that the provider holds anything. A replica whose `last_sync` lags canonical can drop all its data and defend every challenge on the canonical range, repeatably, until it re-syncs. Violates `defendedImpliesEntitled` on **both** instances, so fixing finding 1 does not close it. This one is faithful to the design text (`-implementation.md`, the `Superseded` validity rule explicitly permits `challenged_seq < canonical_end`), which makes it a design finding, not an implementation bug. Scenarios: `supersededEscapeTest`, `supersededEscapeStillTest`.

**3. No challenger tier — every valid defense slashes provider stake.** The design (v2.2/2.3) states that a valid response never touches stake, and that public challengers pay 100% so the provider bears nothing. The pallet's `Challenge` struct has no `authorized` field, though the design's own `Challenge` struct (`-implementation.md`, §Data Structures) specifies one, and `respond_to_challenge` applies the authorized cost-split to everyone — slashing `provider_cost` from stake into the Treasury even on a correct public defense. Repeatable: a crowd of strangers grinds an honest provider's stake down one deposit-fraction at a time, which is the anti-DDoS drain the tier split exists to prevent. Violates `stakeIntactOnDefense`. Scenario: `defenseGrindsStakeTest`.

**4. Missing `leaf_index` bounds check.** No extrinsic validates `target.leaf_index < leaf_count`, so a challenge against a nonexistent leaf is accepted on-chain. Under design semantics an honest provider holding every byte has no valid defense to it — no proof exists, `Superseded` and `Deleted` do not apply — and is full-stake slashed by any stranger for the price of one deposit. Under code semantics this is masked by finding 1, since the stale-leaf proof "defends" it. Two bugs cancelling, which is exactly why `honestNeverSlashed` holds on the code instance and fails on the design instance. Scenarios: `outOfRangeGriefingTest` (design), `outOfRangeMaskedTest` (code).

Three more by inspection, not invariant violations: there is **no `cancel_challenge` extrinsic** although the design specifies challenger cancellation before a response; the maintainer's comment on #265 argues that only `Timeout` should slash, with invalid defenses leaving the challenge open instead — the spec currently models the code's slash-on-invalid, so it is the natural place to explore that change before writing any code; and **a non-provider bucket admin can never mount a `Deleted` defense** — `respond_to_challenge` verifies the admin's deletion signature through `Self::verify_signature`, which resolves the signer's key via the `Providers` map (`impls/signatures.rs`), so unless the admin happens to be a registered storage provider the signature check fails and the response slashes as `InvalidDeletionClaim` even when the deletion is genuine. The MBT driver has to register the admin as a zero-stake provider just to make the spec's `deletedVerifies` semantics reachable on-chain, which is how this one surfaced.

## Finding status

As of 2026-08-28. The spec models `dev`; an open PR changes nothing here until it merges.

| Finding | Fix | State | What must move with it |
| --- | --- | --- | --- |
| 1. Unbound MMR proof | [#301](https://github.com/paritytech/web3-storage/pull/301) | open, mergeable | `challengesCode` flips to `STRICT_PROOF = true`; `anyLeafProofEscapeTest` / `any_leaf_proof_escape` fail and are rewritten as regressions; the driver must submit the challenged leaf's proof and read the `Commitment` that `Challenge` now embeds. |
| 2. `Superseded` escape | none | design finding, no issue filed | Needs a design-owner decision (CODEOWNERS-gated). Untouched by #301 and #330. |
| 3. No challenger tier | [#330](https://github.com/paritytech/web3-storage/pull/330) | open, conflicting with `dev` | Model gains the tier split (authorized cost-split vs. 100% deposit reimbursement) and `stakeIntactOnDefense` should hold; `defenseGrindsStakeTest` / `defense_grinds_stake` fail. `establish_storage_agreement` gains `visibility` defaulting to `Private`, under which `Pub` cannot challenge the primary — the driver must create the bucket `Public`. |
| 4. Missing `leaf_index` bounds check | [#301](https://github.com/paritytech/web3-storage/pull/301) | open, mergeable | Creation-time guard `leaf_index < leaf_count` in the three challenge actions; `outOfRangeMaskedTest` / `out_of_range_masked` fail; `honestNeverSlashed` should then hold on both instances — re-run to confirm. |

The two inspection findings (no `cancel_challenge`; slash-on-invalid vs. timeout-only, per the maintainer comment on #265) have no fix in flight.

Whichever of this spec and a fix PR merges second carries the spec, scenario and driver update in the same change. `cargo test -p pallet-storage-provider-mbt` failing after a fix lands is the intended signal, not a flake.

## Decisions taken, with reasons

**No Choreo.** Choreo is scaffolding for N-process message-passing protocols — per-process local state, a message buffer, the listen/act `cue` split. This protocol is a single sequential state machine: the chain executing totally-ordered extrinsics over shared storage. No messages are exchanged and no per-process view diverges, so Choreo would add ceremony and no interleaving coverage. It would also hurt the MBT work below, since quint-connect needs `Config { state: &[...], nondet: &[...] }` path overrides to reach state nested inside a Choreo environment, whereas flat `var`s need none.

**No `basicSpells`.** It does not resolve against the installed CLI (`QNT013: could not load`) and would have to be vendored into this directory. It would save roughly ten lines — `Option` in place of the `-1` and `NO_COMMIT` sentinels, `min` for `imin`, `mapRemove` for `mapWithout` — in exchange for a ~100-line vendored file. The sentinels are also the better choice for MBT: every Quint `Option` forces an `#[serde(with = "As::<de::Option<_>>")]` annotation in the Rust state struct, while sentinel integers deserialize plainly.

**Instance sizing.** Two providers, `CHALLENGE_TIMEOUT = 3`, `MAX_PER_DEADLINE = 2`, four sequence numbers, three commitments. Issue #265 calls small instances sufficient and that holds — every finding here is logical, and each reproduces in a trace of eight steps or fewer.

**Guards tightened to chain semantics (found by MBT).** Building the replay driver surfaced three places where the model was *looser* than the pallet — spec inaccuracies, not pallet bugs, so the guards were tightened: `replicaSyncC` now requires the synced commitment to equal the current canonical snapshot and to differ from `last_sync` (`confirm_replica_sync` only matches the snapshot root at position 0 and rejects re-syncs); `announceDeregP` requires `now > AGREEMENT_END` and no pending challenges (on-chain the expired agreement row must first be torn down via `claim_expired_agreement`, which enforces both); `doCheckpointC` requires the primary not to have announced deregistration (teardown removes it from the bucket's primary list, after which its checkpoint signature no longer resolves). All invariant results and witness reachability were re-verified unchanged after the tightening.

## Remaining work from the issue

**CI job** (#265 TODO 3) is not written. It needs Node plus `npm i -g @informalsystems/quint` before running the typecheck, both `--main` invariant runs, the witness run, and both `quint test` invocations. Versions come from `.github/env`, never hardcoded. Note the job cannot be green until the findings are triaged, since `allInvariants` currently fails by design — either gate on the four holding invariants and the ten scenarios for now, or hold the workflow until the findings are resolved. Do not add `continue-on-error`.

**Issues per finding** (#265 TODO 4) are not filed. Findings 2 and 3 touch `docs/design/`, which is CODEOWNERS-gated, so they want a design-owner conversation rather than a quiet code patch.

## MBT via quint-connect

**Built and green** — `crates/pallets/storage-provider/mbt/`, a dev-only workspace crate using [`quint-connect`](https://github.com/informalsystems/quint-connect) 0.1.2. It carries its own mock runtime whose Config constants match the spec instance exactly (`ChallengeTimeout = 3`, `DeregisterAnnouncementPeriod = 4`, `ChallengeDeposit = 10`, `MaxChallengesPerDeadline = 2`, stake 100, `SettlementTimeout = 0`, `RequestTimeout = 1`), so spec quantities map onto chain quantities with no scaling. The driver holds a `TestExternalities` plus a real MMR fixture — deterministic chunks for sequences 0–3 built into genuine MMRs, so model roots 1/2/3 map to real `H256` roots with verifying proofs in both directions. `switch!` arms map one-to-one onto extrinsics; `signOffchainC`, `loseLeaf` and `adminDeleteTo` are driver-side ghost bookkeeping; signatures are produced at submission time. For `respondAs` the driver submits the concrete response the model's adjudication assumed — for finding 1 that means the *other* held leaf's real proof, which turns the model-level escape into a demonstration against the real pallet. After every step, `SpecState` (balances, reserved, stake, pending counters, snapshot, open challenges — the chain-visible slice; ghost fields are skipped) is compared against the deserialized ITF state, diffing on divergence.

Two test targets:

```bash
# Deterministic finding scenarios, hand-mirrored from challenges_test.qnt
# (quint test traces carry no mbt metadata, so #[quint_test] replay is not
# possible). Run under plain `cargo test`, no quint CLI needed:
cargo test -p pallet-storage-provider-mbt

# Random-trace replay: 100 traces × 25 steps per run, quint CLI required
# (npm i -g @informalsystems/quint). Gated behind the `mbt` feature so
# `cargo test --workspace` stays green without Node:
cargo test -p pallet-storage-provider-mbt --features mbt
```

Worth stating plainly: because the spec models code semantics, MBT **passes** against today's pallet. Its job is to lock the spec-to-code correspondence so that fixing the findings forces spec, driver and pallet to move together — the finding-shaped tests in `tests/findings.rs` assert the broken behaviour on purpose, exactly like their Quint counterparts. `QUINT_SEED=<seed>` reproduces a failed replay; `QUINT_VERBOSE=1 cargo test ... -- --nocapture` shows per-step actions. The crate is 0.1.x, so expect API churn.

## Ground rules

The spec is the ground truth. Do not edit it to match broken code — if the two disagree, that is a finding to raise, and where the disagreement is with `docs/design/` it goes through a CODEOWNERS-reviewed PR. When the pallet's challenge logic changes, update the spec and re-run everything above before touching the implementation.

Out of scope in the current model, and deliberately so: payment streams and agreement settlement, multi-bucket and multi-agreement interactions, the sweep's per-block PoV budgets (`MAX_SWEEP_SPAN`, `MAX_SWEEP_SLASH_BUDGET`) and its one-block lag, which are collapsed into "advancing past the deadline sweeps it" — a simplification that only delays slashing, the safe direction for every invariant here — along with challenger transaction fees, signature replay, and challenge cancellation. Extending to funds conservation across agreements and checkpoints is listed as follow-up work in the issue itself.
