---
name: design-alignment
description: Verify changes stay aligned with the core design docs (docs/design); any deviation from the documented design must be explicitly flagged
---

The documents under `docs/design/` are the **core design** of this project
(canonical upstream: https://github.com/paritytech/web3-storage/tree/dev/docs/design).
They are canonical and must always be true to what is in the code: code must conform
to the design, and the design docs must accurately describe actual system behavior.
Neither side may drift from the other — the design is never changed implicitly by code.

## What to do

Given a set of changes (local diff against the branch's base — `dev` unless stated
otherwise — or a PR number via `gh pr view` / `gh pr diff`):

1. Identify which design areas the changes touch. `docs/design/` currently holds two
   documents, and most non-trivial changes touch at least one:
   - `scalable-web3-storage.md` — system design, economics, threat model, and the
     rationale behind the game-theoretic guarantees
   - `scalable-web3-storage-implementation.md` — data structures, extrinsic
     signatures, state machines, the provider HTTP API, and runtime constants

   Check the directory listing — new design docs may have been added since this list
   was written.

   `docs/reference/` is derived documentation rather than design — the
   `reference-docs` skill covers it.

2. Optionally consult `docs/drafts/` for background. These documents are unratified,
   WIP, or archives of removed functionality, so they are context rather than spec —
   but they often explain the reasoning behind an area the design docs cover only
   briefly:
   - Checkpoint logic (client or provider) → `CHECKPOINT_PROTOCOL.md`,
     `provider-initiated-checkpoints.md`
   - Provider discovery / capacity / pricing → `marketplace.md`
   - Challenge and slashing economics → `challenge-economics-extensions.md`
   - Precompiles / Solidity examples → `smart-contracts.md`
   - Encryption paths → `CLIENT_SIDE_ENCRYPTION.md`
   - S3 interface metadata → `S3_METADATA_INDEX.md`
   - Layer 1 file system → `L1_design_implementation.md`

   Check each draft's header before relying on it — some are explicitly marked as
   superseded or removed (`provider-initiated-checkpoints.md` archives code deleted
   in #306). Where a draft is useful, cite it as context; reserve
   `⚠️ DESIGN DEVIATION` for contradictions with `docs/design/` itself.

3. Read the relevant sections of the design docs and compare the changed behavior
   against the documented design: state machines, invariants, economic parameters,
   flows, data structures, and guarantees.

4. Check consistency in both directions: code that contradicts the design, and design
   statements the diff makes false. Classify every divergence:
   - **⚠️ DESIGN DEVIATION** — the change makes the code behave differently from the
     documented design. This must be called out explicitly and prominently in your
     output, one flag per deviation, citing the design doc + section it contradicts
     and the code location (`file:line`). Never let a deviation pass silently, and
     never treat the code as the new source of truth.
   - **Aligned** — the change implements or refines what the design already specifies.

5. A design deviation is not automatically wrong — but it requires an explicit,
   deliberate decision. When flagging one, state that the author must either revert
   to the documented design or update the design doc in the same PR with the
   rationale for the change.

## Output

- A short verdict: aligned, or N design deviations flagged.
- Each deviation as a `⚠️ DESIGN DEVIATION` item: what the design says (doc + section),
  what the code now does (`file:line`), and the required action (conform or update
  the design doc with rationale).
