---
name: review
description: Review local changes or a pull request (authoritative review criteria)
---

This skill is the single source of truth for code review criteria in this repository.

If no arguments are passed, review the local changes by looking at the diff between the base branch - `dev` by default - and the current branch.
If arguments are passed, review pull request #$ARGUMENTS by fetching it and seeing its details with `gh pr view` and `gh pr diff`.

When reviewing, analyze for:

1. **Code Quality**
   - Rust idioms and Polkadot SDK patterns
   - Error handling: use `Result` types with meaningful error enums; avoid `unwrap()` and `expect()` in production code (acceptable in tests)
   - Arithmetic safety: use `checked_*`, `saturating_*`, or `wrapping_*` arithmetic to prevent overflow; never use raw arithmetic operators on user-provided values
   - Naming: follow Rust conventions (snake_case for functions/variables, CamelCase for types)
   - Complexity: prefer simple, readable code; avoid over-engineering and premature abstractions
   - Comments should explain **why**, not **how**

2. **FRAME Pallet Standards**
   - Use appropriate storage types (`StorageValue`, `StorageMap`, `StorageDoubleMap`, `CountedStorageMap`)
   - Emit events for all state changes that external observers need to track
   - Define descriptive error types in the pallet's `Error` enum
   - All extrinsics must have accurate weight annotations; update benchmarks when logic changes
   - Use the principle of least privilege for origin checks
   - Be cautious with `on_initialize` and `on_finalize`: they affect block production time and can brick parachains; never panic or do unbounded iteration in them; always benchmark them properly

3. **Security**
   - Runtime code must never panic; use defensive programming
   - Use `BoundedVec`, `BoundedBTreeMap` etc. to prevent unbounded storage growth
   - Validate all user inputs at the entry point
   - Consider requiring deposits for user-created storage items
   - Review unsafe code blocks for soundness
   - Verify access control in pallets uses appropriate origin checks

4. **Performance**
   - Weight/benchmark implications
   - Storage access patterns
   - Unnecessary allocations

5. **Testing**
   - All new functionality requires unit tests
   - Test boundary conditions, error paths, and malicious inputs
   - Complex features need integration tests using `sp-io::TestExternalities`
   - Features affecting weights need benchmark tests

6. **PR Standards**
   - Single responsibility: each PR addresses one concern
   - All CI checks pass (`cargo test`, `cargo clippy`, `cargo fmt`)
   - Code compiles without warnings
   - Public APIs have rustdoc comments

7. **Breaking Changes**
   - API compatibility
   - Migration requirements

8. **Crate Boundaries** (`crates/providers/*`)
   - Prefer these crates not depending on `subxt` or other transport-specific
     clients; where one does, ask whether the seam could be a trait instead
   - The trait belongs in the crate; the subxt-backed implementation belongs in
     `provider-node`, supplied when the node is wired up
   - Rough test: could subxt be swapped by touching mostly provider-node? If not,
     it is worth raising — a suggestion, not a blocking finding

9. **Design Alignment** — invoke the `design-alignment` skill and follow its
   procedure in full; the points below are a summary, not a substitute
   - `docs/design/` is canonical and must stay true to the code
   - Changes must conform to the core design docs in `docs/design/`
   - Any deviation from the documented design must be explicitly flagged as
     `⚠️ DESIGN DEVIATION`, citing the doc/section and the code location; the author
     must either conform to the design or update the design doc in the same PR with
     rationale

10. **Reference Docs Consistency** — invoke the `reference-docs` skill and follow its
    procedure in full; the points below are a summary, not a substitute
    - `docs/reference/` is derived documentation, but it is review-gated and must
      stay true to the code
    - If a change alters a documented flow or makes any statement in `docs/reference/`
      (`EXTRINSICS_REFERENCE.md`, `PAYMENT_CALCULATOR.md`, …) no longer true, the doc
      must be updated in the same change
    - A missing reference-doc update is a blocking finding, flagged as
      `📄 REFERENCE DOC OUT OF DATE`

11. **Stack Shape** — when the PR is part of a stack (its base is not `dev`)
    - Diff each PR against its own base PR, not `dev`
    - Check that every link in the stack is a genuine dependency: the upper PR
      must not compile or make sense without the lower one. Stacking unrelated
      work to avoid a merge wait or a generated-file conflict (bindings,
      metadata, weights) makes the stack unreviewable; flag it
    - Check that the description names the base PR and the intended merge order
    - Propose a concrete restructure (which PRs to retarget to `dev`, merge
      order) rather than just noting the problem. The author does any history
      rewrite

## Writing the findings

Each finding is simple, exact and straight to the point: the problem, where it
is (`file:line`), and the fix or the question. When unsure, say what the
reviewer should check instead of guessing. No fluff, no boilerplate, no praise,
no restating the code.

Present the findings to the user for triage. Never post them to GitHub on your
own (see the code review rules in `CLAUDE.md`).
