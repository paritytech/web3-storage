# Multi-Writer Coordination

> ⚠️ **Unratified draft — needs design review.**
> Proposes two mechanisms for ordering concurrent writers' appends:
> **(A) deterministic writer rotation**, a client-side convention requiring
> no protocol change, and **(B) multi-log buckets**, a protocol extension
> that removes write contention by construction. Motivated by the
> "Concurrent writers (divergence and arbitration)" analysis in
> [scalable-web3-storage-implementation.md](../design/scalable-web3-storage-implementation.md):
> divergence recovery on the current surface is a destructive-write reset,
> so prevention carries all the weight.

## Problem

A bucket's MMR commits to a *sequence*, and the chain does not order
appends — ordering is the writer set's job. Concurrent writers committing
to multiple providers in different orders produce internally-valid but
divergent provider states, with no checkpointable quorum and only a blunt
recovery path.

Ordering appends from multiple writers is total-order broadcast, which is
equivalent to consensus. The key observation that keeps the solution cheap:
**the ordering role carries no safety trust**. A coordinator cannot forge
content (content-addressing), liability (per-provider signatures), or
enforcement (per-writer keys); it can only reorder, delay, or stall — all
detectable, all recoverable. So the problem is ordering fairness and
liveness, not Byzantine agreement, and lightweight mechanisms suffice.

## A. Deterministic writer rotation (no protocol change)

Rotate the sequencer role among the bucket's writers, computed — not
elected — from state every participant already sees:

- **Clock**: the anchor (relay-chain) block, already the denomination for
  every duration in the system.
- **Registry**: the bucket's member list with Writer/Admin roles, already
  on-chain.

```
window          = current_anchor_block / WINDOW_SIZE     (e.g. 20 anchor blocks)
eligible        = bucket members with Writer or Admin role
leader(window)  = argmin over eligible of H(account ‖ window)
```

Rules:

1. Only `leader(window)` issues `/commit` calls during the window, using
   nonces within it and the same provider order everywhere. Other writers
   upload chunks freely at any time (uploads are unordered and
   contention-free — only commit order matters) and either queue their
   `data_roots` for their own leadership window or forward them to the
   current leader to commit on their behalf.
2. Leaders stop committing in the last few blocks of their window (a quiet
   zone), so no commit straddles a window boundary and reaches providers
   under two different leaders.
3. An offline leader costs exactly one idle window: writes queue and the
   next window has a different leader. No election traffic, no timeout
   negotiation, no deadlock.

Properties: zero new endpoints, zero on-chain changes, computable offline
by every participant, tolerant of the flaky clients the protocol already
assumes. Residual limits: a malicious or absent leader stalls its own
window (bounded, detectable); anchor-block observation lag bounds how
tightly windows can be sized.

Precedent: Tendermint/PBFT proposer rotation by round number; this
codebase's removed provider-initiated checkpoints used the same
anchor-block leader election among providers
([provider-initiated-checkpoints.md](./provider-initiated-checkpoints.md)).

## B. Multi-log buckets (protocol extension)

The divergence problem is an artifact of committing to one shared sequence.
The structural fix is per-writer sub-logs under one bucket — the pattern of
Hypercore's Autobase, Scuttlebutt feeds, and the Matrix event graph,
adapted to this system's economics:

```
BucketCommitment
└── writers_root: H256          — root of a small Merkle map
    ├── writer W1 → sub-MMR root, (start_seq, leaf_count)
    ├── writer W2 → sub-MMR root, (start_seq, leaf_count)
    └── …                         (bounded by MaxWritersPerBucket)
```

- **Commits commute.** A writer appends only to its own sub-log, so
  concurrent commits by different writers cannot interleave — providers
  converge regardless of arrival order, and the divergence scenario cannot
  be constructed. Same-writer commits are already serialized by rule
  (single writing process per key).
- **Challenges retarget** from `(leaf_index, chunk_index)` to
  `(writer, leaf_index, chunk_index)`; the extension/consistency check and
  the peaks mechanics apply per sub-log unchanged.
- **Deletion and compaction** become per-sub-log (`start_seq` per writer),
  which also scopes quota accounting naturally.
- **Order-sensitive views** (a shared directory) apply a deterministic
  linearization at read time — e.g. sort entries by
  `(anchor-block window, writer)` — computed identically by everyone with
  no coordination. Most Layer-1 state does not need a total order at all.
- A single-writer bucket is the trivial special case (map of size one), so
  existing behavior is preserved.

Costs and open questions:

- `CommitmentPayload` and `Commitment` change shape (version bump), and
  `checkpoint` / `challenge_*` extrinsics change accordingly — weights and
  benchmarks included.
- `MaxWritersPerBucket` bound and the map's on-chain representation.
- Whether the snapshot's provider bitfield and `min_providers` semantics
  stay per-bucket (likely yes — providers still sign the whole
  `writers_root`).
- Migration for existing buckets (or none: new bucket type / flag at
  creation).
- Whether directory semantics at Layer 1 want last-writer-wins per path or
  explicit merge — interacts with the layered-architecture decision
  ([#51](https://github.com/paritytech/web3-storage/issues/51)).

## Decision needed

- [ ] **A** — adopt writer rotation as the documented SDK convention for
      multi-writer buckets (client behavior only; canonical docs reference
      it as the recommended sequencer realization).
- [ ] **B** — review multi-log buckets; either canonicalize into
      `docs/design/` with a corrected payload spec and implement, or record
      why (A) alone is sufficient.

## References

- Tendermint proposer rotation; PBFT views (deterministic leadership by
  round).
- Hypercore Autobase (multiple single-writer input logs + deterministic
  linearized view); Secure Scuttlebutt (per-actor append-only feeds);
  Matrix event DAG + state resolution.
- Certificate Transparency (append-only consistency proofs, as already used
  by the extension check).
