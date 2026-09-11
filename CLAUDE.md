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
- New or changed extrinsics need fresh benchmarks. Do not run `/cmd bench`
  yourself; tell the user the PR needs re-benching and let them trigger it.
  A placeholder weight is fine in the meantime if it is marked
  `// TODO: needs re-benchmarking`.
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
- Review criteria, the finding format and the stacked-PR checks live in the
  `/review` skill. Use it for every review.

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

## Rules and harness

### IMPORTANT: Minimize AI slop; use plain, simple language
Do not use invented shorthands or heavy jargon. Say what something actually is.
Never use metaphors or rhetorical flourishes. Never anthropomorphize.
For example, a file does not "sit" in a directory; it "exists" there ("sit" implies it could also "stand"). A problem does not "bite"; it "occurs" (a problem has no mouth).
No proverb symmetry ("teams change, topics stay"). No balanced contrast ("is a copy, not a rewrite"). No novelist's diction ("enters", "the latter case"). No wordplay.
Be concrete. Do not use vague imperatives like "name them", "belongs elsewhere" or "that's all it takes".
Never use fancy vocabulary. Use dry, technical, non-literary words. For example:
  - do not say "carry"; say "continue"
  - do not say "load-bearing"; say "critical"
  - do not say "survives"; say "remains"
  - do not say "asked"; say "requested"
  - do not say "refuses"; say "rejects"
  - do not say "holds"; say "contains"
Use direct, dry, technical language. Avoid phrases and names that read as sentences or narrate. For example:
  - do not say "asked to think"; say "thinking enabled"
  - do not say "what was checked, not assumed"; say "what I checked"
  - do not say "room to answer"; say "remaining capacity"
  - do not say "where it stopped"; say "stopping point"
  - do not say "for a reason worth writing down"; say "for an important reason"
  - do not say "was never written down"; say "was never documented"
Write like a software engineer with no literary skill.
Never use abstract, soft phrasing that does not say what something is, or that only passively refers to something.
Never use passive voice. Use active voice. For example, do not say "the last message wasn't written down"; say "the last message doesn't exist".
Cut filler. Never editorialize. Use simple structure and simple vocabulary.
This applies to everything you output: messages to the user, strings in code, method names, variable names, commit messages, and your own notes and status files.
Do not match existing style when it disagrees with these guidelines.

### IMPORTANT: Use dry, simple, concrete, technical language in code
When writing code all of the "minimize AI slop" rules apply.
Name things in the simplest, purely technical language.
The following words are FORBIDDEN and should NEVER be used in code nor in any message in code: `ran`, `landed`, `land`, `given`, `give`, `settled`, `settle`, `held`, `holds`, `holding`, `says`, `names`, `named`, etc.
Never use past participle in code.
Always name things in *concrete* terms, for example:
  - do not write "written_at"; write "write_timestamp"

### IMPORTANT: Fix the root cause, not the symptom
When fixing a bug, figure out what is its root cause, not just what directly caused it.
Is the issue you're fixing a consequence of a particular architectural decision?
Is there a more *fundamental* fix you could apply which not only fixes this issue, but also either fixes similar issues, or prevents the issue from reappearing in the future?
Figure out *if* there is a fundamental root cause to what you're fixing, and what that root cause is.
NEVER patch the symptoms when a root cause exists.
When in doubt, ask the user to decide.
