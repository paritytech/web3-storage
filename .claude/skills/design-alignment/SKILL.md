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

Given a set of changes (local diff against `main` by default, or a PR number via
`gh pr view` / `gh pr diff`):

1. Identify which design areas the changes touch. Map changed files to the relevant
   design docs, e.g.:
   - Pallet / runtime / challenge / slashing / stake logic → `scalable-web3-storage.md`,
     `scalable-web3-storage-implementation.md`
   - Extrinsic flows and sequencing → `EXECUTION_FLOWS.md`
   - Checkpoint logic (client or provider) → `CHECKPOINT_PROTOCOL.md`,
     `provider-initiated-checkpoints.md`
   - Provider discovery / capacity / pricing → `marketplace.md`
   - Precompiles / Solidity examples → `smart-contracts.md`
   - Encryption paths → `CLIENT_SIDE_ENCRYPTION.md`
   - S3 interface metadata → `S3_METADATA_INDEX.md`
   - Storage backend choices → `storage-db-decision-notes.md`

   Check the directory listing — new design docs may have been added since this list
   was written.

2. Read the relevant sections of those docs and compare the changed behavior against
   the documented design: state machines, invariants, economic parameters, flows,
   data structures, and guarantees.

3. Check consistency in both directions: code that contradicts the design, and design
   statements the diff makes false. Classify every divergence:
   - **⚠️ DESIGN DEVIATION** — the change makes the code behave differently from the
     documented design. This must be called out explicitly and prominently in your
     output, one flag per deviation, citing the design doc + section it contradicts
     and the code location (`file:line`). Never let a deviation pass silently, and
     never treat the code as the new source of truth.
   - **Aligned** — the change implements or refines what the design already specifies.

4. A design deviation is not automatically wrong — but it requires an explicit,
   deliberate decision. When flagging one, state that the author must either revert
   to the documented design or update the design doc in the same PR with the
   rationale for the change.

## Output

- A short verdict: aligned, or N design deviations flagged.
- Each deviation as a `⚠️ DESIGN DEVIATION` item: what the design says (doc + section),
  what the code now does (`file:line`), and the required action (conform or update
  the design doc with rationale).
