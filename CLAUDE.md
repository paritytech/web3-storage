# CLAUDE.md - Scalable Web3 Storage

Rules for agents working in this repo.

**This file contains behavioral rules and pointers only — never facts about
the system.** Facts (parameters, mechanisms, flows, APIs) live in `docs/` and
the code, per the source-of-truth map below. Do not restate them here: this
file is loaded into every session but is not CODEOWNERS-gated, so any copy of
a design fact placed here becomes an unreviewed, drift-prone shadow spec.
Link, don't copy.

Conventions for agent-written text and PRs are in `AGENTS.md`, imported here
so every tool reads the same text:

@AGENTS.md

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

**Code review rules:**
- NEVER post review findings (PR reviews, inline or issue comments) to
  GitHub on your own. Present them to the human reviewer for triage first
  and post only the ones they approve, when they ask.
- Review criteria, the finding format and the stacked-PR checks live in the
  `/review` skill. Use it for every review.

**Conventions (in `AGENTS.md`, imported above):**
- Pull requests: base branch, single responsibility, regenerated files,
  benchmarks, description structure, stacking.
- Writing: documentation rules, plain language, root-cause fixes.
- Code: Rust workspace and Cargo dependency rules, JS/TS via `polkadot-api`.

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

## AI review bot

Comment `/aireview` on a PR to get an advisory review from Vertex AI
(`.github/workflows/vertex-ai-review.yml`). It is not a substitute for human
review.
