---
name: reference-docs
description: Verify docs/reference stays truthful — if a change alters a documented flow or invalidates anything in the reference docs, the docs must be updated in the same change
---

The documents under `docs/reference/` are user-facing reference material
(canonical upstream: https://github.com/paritytech/web3-storage/tree/dev/docs/reference).
They must never drift from the code: if a change alters a documented flow, or makes
any statement in a reference doc no longer true, the doc must be updated in the same
change.

Current reference docs (check the directory — more may have been added):

- `EXTRINSICS_REFERENCE.md` — the complete blockchain API: every extrinsic, its
  parameters, events, and errors.
- `EXECUTION_FLOWS.md` — sequence diagrams and step-by-step flows for every
  extrinsic, including the off-chain HTTP calls around them.
- `PAYMENT_CALCULATOR.md` — payment formulas, worked examples, and parameter values.

## What to do

Given a set of changes (local diff against the branch's base — `dev` unless stated
otherwise — or a PR number via `gh pr view` / `gh pr diff`):

1. Detect reference-impacting changes. In particular:
   - Added / removed / renamed extrinsics, or changed signatures, parameters,
     events, or `Error` variants in `crates/pallets/**` → `EXTRINSICS_REFERENCE.md`,
     and `EXECUTION_FLOWS.md` when the call sequence or its surrounding HTTP
     steps change
   - Changes to payment math, pricing, stake formulas, or the runtime constants they
     depend on (`UNIT`, `MinProviderStake`, `MinStakePerByte`, timeouts, checkpoint
     reward/penalty in `runtimes/`) → `PAYMENT_CALCULATOR.md`
   - Any behavior change that contradicts an example, table, or statement in a
     reference doc.

2. For each impact, open the reference doc and verify whether the diff already
   updates it to match the new behavior.

3. Also do the reverse check: for reference docs touched by the diff, confirm the
   documented behavior matches what the code actually does.

4. Report:
   - **📄 REFERENCE DOC OUT OF DATE** — the change alters documented behavior but the
     corresponding reference doc was not updated (or was updated incorrectly). Cite
     the code change (`file:line`), the doc and the specific section/statement that
     is now false, and what it should say.
   - **OK** — no reference impact, or the docs were updated consistently.

A change that invalidates a reference doc must not merge without the doc update —
treat a missing update as a blocking finding, not a nit.

## Output

- A short verdict: reference docs consistent, or N stale-doc findings.
- Each finding as a `📄 REFERENCE DOC OUT OF DATE` item with code location, doc
  section, and the required correction.
