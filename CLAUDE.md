# CLAUDE.md - Scalable Web3 Storage

Rules for agents working in this repo.

**This file contains behavioral rules and pointers only — never facts about
the system.** Facts (parameters, mechanisms, flows, APIs) live in `docs/` and
the code, per the source-of-truth map below. Do not restate them here: this
file is loaded into every session but is not CODEOWNERS-gated, so any copy of
a design fact placed here becomes an unreviewed, drift-prone shadow spec.
Link, don't copy.

## Design & spec discipline (read this first)

- `docs/design/` is the **canonical, review-gated source of truth** (enforced
  by `.github/CODEOWNERS`). Reason and implement *from* it; treat it as the
  spec.
- **Before writing or changing pallet, runtime, or provider behavior, read
  the relevant section of `docs/design/`** — don't rely on summaries of it
  found elsewhere. For non-trivial changes, run `/design-alignment` before
  committing.
- **Validate code against the design. On any divergence, or anything in the
  design that looks wrong or vulnerable, stop and flag**: open or reference
  an issue and ping the design owner. Never quietly edit the design to match
  the code or fix the code on assumptions. Design changes go through a PR
  reviewed per `.github/CODEOWNERS`.
- **`docs/reference/`** is *derived* documentation, but it is **review-gated**
  (per `.github/CODEOWNERS`) and must stay true to the code. When you change
  behavior, **update the relevant `reference/` doc in the same change** (run
  `/reference-docs` to check).
- **`docs/drafts/`** is unratified / WIP — don't treat it as authoritative or
  reason from it as if it were the spec.

## Source-of-truth map

| To know about… | Read… |
|---|---|
| What the system is, architecture, directory layout | root [`README.md`](README.md), [`docs/design/scalable-web3-storage.md`](docs/design/scalable-web3-storage.md) |
| Mechanisms: agreements, checkpoints, challenges, slashing, MMR, anchor clock, replica sync | [`docs/design/scalable-web3-storage-implementation.md`](docs/design/scalable-web3-storage-implementation.md) |
| Runtime parameter values (stakes, timeouts, decimals) | `runtimes/web3-storage-local/src/storage.rs` — the code is the value; the design doc has the rationale |
| Extrinsics API, execution flows, payment math | [`docs/reference/`](docs/reference/) |
| Layer 1 file system (drives, manifests, commit strategies) | [`docs/filesystems/README.md`](docs/filesystems/README.md) |
| WIP designs: marketplace/discovery, checkpoint protocol, smart contracts, encryption | [`docs/drafts/`](docs/drafts/) — **not authoritative** |
| Review criteria (Parity Standards) | the `/review` skill — authoritative; not restated here |
| TypeScript SDK layering, tx semantics, PAPI patterns | [`packages/sdk/README.md`](packages/sdk/README.md) |
| Upstream FRAME / Cumulus / XCM | [Polkadot SDK docs](https://paritytech.github.io/polkadot-sdk/) |

One cross-cutting convention worth knowing before touching any on-chain
duration: **all pallet durations are measured in anchor (relay-chain) blocks,
not parachain blocks** — see the anchor-clock section of the implementation
design doc.

## Agent rules

**Git commit rules:**
- NEVER add Co-Authored-By lines to commits
- NEVER use git rebase
- NEVER force-push (`git push --force`, `-f`, or `--force-with-lease`). This
  matters most once a PR is marked ready for review or has review activity:
  rewriting its history destroys the reviewers' "changes since your last
  review" view and detaches their inline comments. Address feedback with new
  commits on top; if history genuinely has to be rewritten, stop and ask the
  user instead of doing it.

**Documentation rules:**
- Every doc you add or edit (rustdoc, READMEs, design text, code comments,
  PR and issue text) is simple and straight to the point. Say what the
  reader needs, once. No fluff, no restating the code or the diff, no
  boilerplate sections, no marketing tone.
- Public APIs need rustdoc.

**Pull request rules:**
- ALWAYS open pull requests against the repository's default branch (`dev`).
- Single responsibility per PR; all CI checks must pass.
- Regenerated files (subxt/PAPI bindings, metadata, weights) go in their own
  commit so reviewers can skip them.
- New or changed extrinsics are benchmarked before review: run `/cmd bench`
  on the PR. Never leave hand-written estimates in a runtime weight file.
- PR description: one or two sentences on what the PR does and why, then
  bulleted sections as needed: **Changes**, **Cleanup**, **Follow-ups**,
  **Open questions**. Skip empty sections.
- Stacked PRs only when the upper PR genuinely depends on the lower one, and
  each PR in the stack is still a single, self-contained, reviewable change.
  Do NOT stack unrelated work (a feature on a bug fix on a docs fix) just to
  avoid waiting for a merge or to dodge conflicts in generated files
  (bindings, metadata, weights) — that makes the stack unreviewable. The
  description names the base PR and the intended merge order and is kept
  current as the PR changes.

**Code review rules:**
- NEVER post review findings (PR reviews, inline or issue comments) to
  GitHub on your own. Present them to the human reviewer for triage first
  and post only the ones they approve, when they ask.
- Review comments are simple, exact and straight to the point: the problem,
  where it is (`file:line`), and the fix or the question. One finding per
  comment. No fluff, no boilerplate, no praise, no restating the code.
- When the PR under review is part of a stack, review the stack shape too:
  diff each PR against its own base PR (not `dev`), check that every link
  is a genuine dependency per the stacked-PR rule above, and flag stacking
  that only avoids a merge wait or a generated-file conflict. Propose a
  concrete restructure (which PRs should be retargeted to `dev`, merge
  order) rather than just noting the problem. The author does any history
  rewrite; the git rules above still bind the agent.

**Workspace crate rules:**
- When adding, splitting out, or renaming a workspace member crate, ALWAYS
  classify it in `scripts/coverage.sh`: add it to `COV_PACKAGES` (measured)
  or `COV_SKIP_PACKAGES` (skipped, with a reason comment). CI's coverage job
  fails on any unclassified member.
- Prefer keeping `crates/providers/*` free of `subxt`: express what the crate
  needs as a trait and let `provider-node` supply the subxt-backed
  implementation, so swapping the chain client stays a provider-node change.

**Cargo dependency rules:**
- ALWAYS declare external dependencies in the root `[workspace.dependencies]`
  and inherit them in crates via `{ workspace = true }`. Never add
  inline-versioned dependencies (e.g. `foo = "1.2"`) to a crate's
  `Cargo.toml`.
- On the inheriting line you may only add `features` (additive) and
  `optional`; per Cargo, `version` and `default-features` cannot appear
  there, so set `default-features` in the workspace declaration (e.g.
  `hex = { version = "0.4", default-features = false }`).

**Automatic formatting:**
- ALWAYS run `/format` after generating or modifying Rust code, and before
  creating any git commit (Rust + TOML formatting, feature-propagation lint,
  clippy)

## Commands

```bash
just setup           # one-time: download binaries, build
just build           # cargo build --release
cargo test           # all tests (or -p <crate> for one)
just fs-test-all     # Layer 1: primitives + pallet + client tests
cargo clippy --all-targets --all-features --workspace -- -D warnings
```

Formatting (what `/format` runs):

```bash
cargo +nightly fmt --all
taplo format --check --config .config/taplo.toml
zepter run --config .config/zepter.yaml
```

Local network and demos (three terminals):

```bash
just start-chain     # Terminal 1 — zombienet: relay + parachain
just start-provider  # Terminal 2 — provider HTTP node
just demo            # Terminal 3 — Layer-0 PAPI flow (registers provider,
                     #   opens an agreement, exercises challenges)
just fs-demo-ci      # Terminal 3 — Layer-1 file-system flow
just s3-demo-ci      # Terminal 3 — Layer-1 S3 flow
just sc-demo         # Terminal 3 — smart-contract marketplace flow
just health          # provider health check
bash scripts/check-chain.sh  # relay + parachain + current block
```

Network URLs: relay `ws://127.0.0.1:9900`, parachain `ws://127.0.0.1:2222`,
provider HTTP `http://localhost:3333`. Inspect via
[polkadot.js Apps](https://polkadot.js.org/apps/?rpc=ws://127.0.0.1:2222).

When the user says **"run locally"** (or "run the UIs", "start the UIs",
"spin up the UIs"), invoke the `run-local-uis` project skill — it starts all
six `user-interfaces/` apps on their canonical ports with Vite HMR (landing
5176, drive-ui 5174, provider 5175, s3-ui 5177, photos 5178, explorer 5179).

## JS/TS: use `polkadot-api`, never `@polkadot/*`

For any JavaScript or TypeScript code in this repo (demos, scripts, tooling,
SDKs), talk to the chain through `polkadot-api` (PAPI). Do NOT introduce
`@polkadot/keyring`, `@polkadot/util-crypto`, `@polkadot/util`,
`@polkadot/api`, or any other `@polkadot/*` package — they duplicate
functionality PAPI already provides, drag in 20+ transitive deps, and force
`cryptoWaitReady()` awaits everywhere.

| Need | Use |
| --- | --- |
| Chain client + typed API | `polkadot-api` (`createClient`; `getWsProvider` from `polkadot-api/ws`) |
| Signer wrapper | `getPolkadotSigner` from `polkadot-api/signer` |
| SCALE / `Binary` / `Enum` | `import { Binary, Enum } from "polkadot-api"` — NOT `@polkadot-api/substrate-bindings` (its 0.20+ `Binary` is a codec helper without `fromBytes`/`asBytes`) |
| Sr25519 key derivation (`//Alice`) | `sr25519CreateDerive` from `@polkadot-labs/hdkd` + `DEV_PHRASE` + `entropyToMiniSecret` + `mnemonicToEntropy` from `@polkadot-labs/hdkd-helpers` |
| SS58 encode / decode | `ss58Address` / `ss58Decode` from `@polkadot-labs/hdkd-helpers` |
| blake2-256 hashing | `blake2b256` from `@polkadot-labs/hdkd-helpers` |
| `cryptoWaitReady()` | Not needed — hdkd is synchronous; delete the import and the await |

In-repo code should not hand-roll these patterns: the workspace package
`@web3-storage/sdk` (`packages/sdk`) already provides `connect`,
`makeSigner`, the `Alice..Ferdie` dev signers, `submitTx`,
`watchValue`-based waits, and typed wrappers for every pallet extrinsic.
Import from it instead. The canonical signer/derive pattern and the SS58
address-comparison gotcha (`ss58Address` defaults to prefix 42 while PAPI
surfaces the runtime prefix — compare raw bytes via `ss58Decode`, never
strings) are documented in [`packages/sdk/README.md`](packages/sdk/README.md).

## AI review bot

Comment `/aireview` on a PR to get an advisory review from Vertex AI
(`.github/workflows/vertex-ai-review.yml`). It is not a substitute for human
review.
