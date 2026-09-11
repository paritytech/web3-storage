# RFC: Bucket transfer between providers (Draft)

> **Draft — needs triage.** Analysis and proposal, **not implemented**. Written
> against `dev` at 67ce0738 (2026-09-11). Design changes go through a
> `docs/design/` PR per `.github/CODEOWNERS`.

## Story

A dApp operator owns bucket `B`. The dApp's users write into `B` (as `Writer`
members, through a shared writer key, or through a `pallet_revive` contract).
The operator never had a local copy of the data and cannot ask the users to
upload it again.

`B` has one primary provider `P1` under a 1-year agreement. Before it expires,
`B` must continue on provider `P2`:

1. `P2` becomes a **primary**. Replicas are read-only and the dApp keeps
   writing.
2. `P2` obtains the full content of `B` from `P1` or a replica. Nobody
   re-uploads.
3. The operator gets on-chain evidence that `P2` has the data before `P1`
   leaves.
4. `bucket_id`, members, snapshot history and every external reference stay
   valid.

Same story, other triggers: `P1` winds down (#281), blocks extensions, raises
prices, or is slashed.

## Designed vs. implemented

| Topic | Design | Code |
|---|---|---|
| Several primaries per bucket | Yes. Primaries are "Added only by bucket admin", up to ~5 (design doc, "Buckets: Stable Identity"). Use cases start with "Add 2-3 diverse providers". | Shape exists: `primary_providers` (`MaxPrimaryProviders` = 5), N-signature `checkpoint`, positional signer bitfield, removal on expiry. |
| Adding a primary to an existing bucket | **Not specified.** The only primary flow is `establish_storage_agreement`, which creates the bucket. | **Missing.** `establish_storage_agreement` rejects `terms.bucket_id = Some(_)` and always creates a bucket (`impls/agreements.rs:151`, `:218`). Tests and benchmarks write the multi-primary shape directly into storage. |
| Data movement between primaries | "Primary providers don't sync with each other. Clients are responsible for uploading to each primary" (impl doc, "Multi-Provider Coordination", line 110). A lagging primary syncs by "client re-uploads" (line 116). | None. |
| Replica sync | Autonomous, from primaries or other replicas; `confirm_replica_sync`; challengeable. Read-only. | Implemented (`provider-node/src/replica_sync*.rs`). The coordinator syncs only `Replica`-role agreements (`subxt_client.rs:527`) and fetches only from primaries. Sync endpoints `/mmr_peaks`, `/mmr_subtree`, `/fetch_nodes` are role-agnostic. |
| Liability of a primary | Only for snapshots it signed (bit in `primary_signers`). | `challenge_checkpoint` requires the bit (`lib.rs:2230`). `extend_checkpoint` adds a late signature but needs a writer/admin origin and drops the bit silently if the signer's index is beyond the stored bitfield (`lib.rs:2180-2182`). |
| Continuation on one provider | `extend_agreement` before expiry. Binding to expiry, no auto-renew. | Implemented. Rejects expired agreements (`lib.rs:1942`). |
| After expiry | "Provider is no longer bound to store data." No retention window. "Liable for snapshots they signed until superseded." | All three challenge extrinsics require `anchor < expires_at`. Liability ends at the expiry block. `SettlementTimeout` (24h) is a payment window. |
| `transfer_agreement_ownership` | Changes the owner account of one agreement. Same provider, same bucket. | PR #414 (open) implements it. It moves the payer, not the provider or the data, so it does not cover this story. |
| Private buckets | Primaries serve reads only to members; provider accounts are not members. | Read and sync endpoints are unauthenticated (#383). |
| Layer 1 | — | `DriveInfo.expires_at` / `max_capacity` are write-once copies of the first terms. `delete_drive` → `cleanup_bucket_internal` ends every agreement early with pro-rated refunds, replicas included, no role check. |

Net: the story is not possible today. `P2` can only get a new bucket, and only
the users' clients could fill it.

## Direction

Make "primary" a role the admin can add to an existing bucket. Let a joining
provider obtain the data from any current holder with the sync protocol that
replicas already use. The bucket is never transferred; the provider set
changes. General form: keep N agreements live, replace one at a time.
End-of-life migration is the N=1 case.

### Proposal

**On chain**

1. `add_primary_agreement(bucket_id, provider, terms, sig)`, admin origin.
   Redeems provider-signed primary terms with `terms.bucket_id =
   Some(bucket_id)`. Same validation as `establish_storage_agreement`
   (signature, nonce window, `accepting_primary`, duration, capacity, stake)
   plus: bucket exists, no agreement for this provider on it,
   `primary_providers` not full. Accepts a bucket with zero primaries. Holds
   payment, pushes to `primary_providers`, resizes `snapshot.primary_signers`
   to cover the new index, emits `PrimaryProviderAdded`.
2. Provider-origin attestation of the current snapshot: widen
   `extend_checkpoint` to accept a primary signing for itself, or add
   `confirm_primary_sync(bucket_id, signature)`. Required: the operator may
   be offline, and without the bit nobody can challenge `P2`.
3. Leaving is unchanged: `finalize_agreement` removes the primary and clears
   its bit.

**Provider node**

4. Catch-up duty for any agreement, regardless of role, whose local MMR root
   differs from the chain snapshot root. Reuse `ReplicaSync::sync_from_primary`.
   Sources: every provider with a live agreement on the bucket, replicas
   included. No sync payment for primaries.
5. On completion, sign the snapshot commitment and submit it (step 2).

**Client / SDK / UI**

6. "Add provider" and "Replace provider" flows: discover, negotiate over
   `POST /negotiate`, call `add_primary_agreement`, watch `P2`'s sync status
   and signer bit, stop extending `P1`. Replace the `upload_replicated` stub
   in `clients/storage/src/storage_user.rs`.
7. Writers upload to every primary from the join block. The dApp write path
   must resolve `primary_providers` from chain per write, not from a
   configured URL. The client-side Checkpoint Manager
   (`docs/drafts/CHECKPOINT_PROTOCOL.md`) already reads the list from chain.

```
operator                  chain                       P1            P2
  | add_primary_agreement(B, P2, terms, sig) -->|
  |                          primary_providers = [P1, P2], bitfield resized
  |                                             |  P2: read snapshot root from chain
  |                                             |  P2 <-- /mmr_peaks, /fetch_nodes -- P1 (or replica)
  |                                             |  P2 verifies every node against the root
  |                                             |  P2 --> confirm_primary_sync --> bit set
  | writes go to P1 and P2 until T
  | at T: P1 expires, removed, bit cleared      primary_providers = [P2]
```

`P2`'s liability starts at the bit. Before that it is paid and not slashable,
like any fresh primary between agreement start and first checkpoint.

### Alternatives

- **Promote a replica in place**: `promote_replica_to_primary` converts a
  live replica agreement with a fresh primary quote. Proof of possession is
  the existing `last_sync`. Cost: mutates a live agreement (duration, price,
  quota, role) and needs a mid-term settlement of the replica part. The
  design treats agreement terms as immutable.
- **Client re-upload**: the operator downloads from `P1` and uploads to `P2`.
  Fails the story: the operator may not be allowed to read per-user data and
  must not be the bottleneck for every byte.

### Hint: replica as warm standby

Fund a replica of `B` on `P2` from day one (the design's own redundancy
answer). Before `P1` expires, join `P2` as primary: the data is local, catch-up
is a no-op, attestation is immediate. Constraint: agreements are keyed by
`(bucket, provider)`, so `P2` cannot be replica and primary at once. Either
end the replica agreement first (leaves a window without liability), or key
agreements by `(bucket, provider, role)` so both can coexist. The second is a
storage-layout change with no new semantics and makes the standby seamless.

## Gaps

Transfer:

- **G1** No way to add a primary to an existing bucket. Also the reason a
  bucket whose sole primary expired, was slashed or was early-terminated can
  never be written again: `checkpoint` has no eligible signer.
- **G2** No primary-to-primary data movement; the sync loop is gated to the
  replica role and fetches only from primaries.
- **G3** No provider-origin attestation; bitfield sized at checkpoint time.
- **G4** Private bucket: the joiner is not a member, so no honest source
  unless the admin adds it as `Reader` or a replica exists.
- **G5** Layer 1 mirrors `expires_at` / `max_capacity` from the first terms.

Continuity:

- **C1** Paid is not liable. A second primary that never signs is paid and
  never slashable. `min_providers = 2` forces both signatures but one dead
  primary then blocks all checkpoints until the admin lowers it or
  early-terminates (full payment). No intermediate setting.
- **C2** Renewal needs a live, funded owner at the expiry block. No auto-renew,
  no prepaid pool. Missing the block on the sole primary triggers G1.
- **C3** No notice obligation: the provider can block extensions one block
  before expiry.
- **C4** Just-in-time migration depends on the leaver serving. Standing
  redundancy (warm standby) is the shape that does not.
- **C5** `delete_drive` ends every agreement on the bucket early with
  refunds, including third-party replicas. Contradicts "no early
  cancellation for clients" and "replicas cannot be early-terminated".
- **C6** Replica attestation must match the current root or one of six
  historical roots (oldest ~113 anchor blocks). A large, frequently
  checkpointed bucket may never let a replica confirm.
- **C7** Nobody challenges automatically; an offline provider stays paid
  until a human challenges, then 48h pass.

Retention:

- No window after `expires_at`. The provider may delete at the expiry block.
  A transfer must complete before it. If a migration window is wanted, the
  smallest form is a `RetentionPeriod` after `expires_at` during which
  challenges stay valid and the agreement cannot be settled, priced into the
  quote.

Economics (inherent, document only):

- Overlap payment to `P1` and `P2` for the sync window.
- Serving the bulk fetch is unpaid (read incentives not implemented); the
  operator's authorized-tier challenge against `P1` is the existing lever.

## Drifts found (flag, not fixed here)

1. `extend_checkpoint` drops a signer bit beyond the stored bitfield and still
   emits `BucketCheckpointed` for that provider (`lib.rs:2180-2182`).
2. "Snapshot liability remains until superseded" does not apply after expiry
   in code.
3. `delete_drive` early-terminates replicas with refunds (C5).
4. The provider node accepts `PUT /node` and `POST /commit` for any bucket a
   writer authenticates for, without checking its own agreement or role
   (`provider-node/src/api.rs:262-303`). Quota part is #382; the role check
   is unfiled.
5. The design's use cases assume a join path ("Add 2-3 diverse providers")
   that the implementation doc never specifies.
6. `transfer_agreement_ownership` was specified and unimplemented (DRIFT-015
   in #376); PR #414 closes it.

## Open questions

1. Attestation: widen `extend_checkpoint` or add `confirm_primary_sync`?
2. Reuse `PRIMARY_TERM_CONTEXT` with `bucket_id = Some(_)` as the join
   discriminator, or a distinct `"primary-join-v1:"` context?
3. Amend "Primary providers don't sync with each other" to "primaries may
   catch up from any provider with a live agreement; clients deliver new
   writes to every primary"?
4. Key agreements by `(bucket, provider, role)` to allow the warm standby?
5. `RetentionPeriod` after expiry: yes or no?
6. Layer 1: derive mirrored fields from Layer 0, or add update paths?
7. After #414, a drive owner and its agreement owner can diverge. Does
   Layer 1 need a matching drive transfer? (Open question in #414.)

## Related

- Migration and wind-down: #65 §4, #281, #107.
- Multi-provider checkpoints: `docs/drafts/CHECKPOINT_PROTOCOL.md`, #332,
  #388, #302.
- Multi-writer and dApp buckets: #134, #133, `docs/drafts/smart-contracts.md`,
  design doc "Media in Chat". No dedicated multi-writer issue exists.
- Ownership transfer: #414, #376 (DRIFT-015), #403.
- Provider-node gaps: #382, #383, #310.
- `docs/drafts/marketplace.md`, "Automatic Data Migration".
