# Bucket transfer between providers: design gaps (Draft)

> **Draft — needs triage.** Gap summary for discussion with the design owner.
> No proposal in this document. Written against `dev` at 67ce0738
> (2026-09-11). Design changes go through a `docs/design/` PR per
> `.github/CODEOWNERS`.

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
4. `bucket_id` stays the same.

Same story, other triggers: `P1` winds down (#281), blocks extensions, raises
prices, or is slashed.

### Why `bucket_id` must not change

The design makes `bucket_id` the stable reference (design doc, "Bucket
Addressing": "The bucket_id is the stable anchor. Providers can come and go,
but the bucket_id never changes. Applications reference buckets, not
providers."). Everything downstream depends on that:

- `bucket://<id>/...` URIs in applications and documents.
- DNS TXT records and DotNS names that resolve to `bucket_id` plus a leaf
  index (design doc, "Public Website" use case).
- `DriveInfo.bucket_id` and the S3 bucket mapping in Layer 1.
- Existing replica agreements, which are keyed by `bucket_id`.
- Members and roles, `min_providers`, `frozen_start_seq`, visibility, the
  snapshot and its history.
- Smart contracts that store a `bucket_id`.

Today the only way to get a second provider under contract is
`establish_storage_agreement`, which creates a new bucket. So "new agreement
with `P2`" means a new `bucket_id`, a full re-upload by whoever has the data,
re-creating members and settings, re-funding replicas, and updating every
reference above. The design's own promise ("the bucket_id never changes")
is not reachable through the extrinsics that exist.

**Net: the story is not possible today.** `P2` can only get a new bucket, and
only the users' clients could fill it. What the story needs is a bucket-level
provider change: either the bucket moves to another provider under a new
agreement, or, at minimum, a primary provider can be added to an existing
bucket. Neither is specified in the design.

### Sub-questions

**Q1. Who owns the user data, and who pays for it?** The operator has an
agreement with `P1` for the SPA. The SPA then produces data for and by each
user. Options, none of them specified:

- One operator bucket, users as `Writer` members. Bounded by `MaxMembers`
  (100). Operator pays and controls. Users' data dies with the operator's
  agreement.
- One operator bucket, shared writer key or a contract as the writer
  (`docs/drafts/smart-contracts.md`). No member bound. Same ownership shape.
- One bucket per user, user-owned agreement. Follows the Web3 principle that
  the user pays for and controls their own data. Requires every user to hold
  funds and sign extrinsics, and the SPA to discover each user's bucket from
  chain. Moves the same continuity problem to every user.
- Contract-owned per-user buckets (token-gated drive pattern in the
  smart-contracts draft).

Does every user need their own agreement? Which shape is intended for a dApp?

**Q2. Keeping the SPA live past `expires_at`.** The SPA bucket is static and
public. Read-only continuation works today: anyone can fund and extend a
replica of a frozen public bucket, visitors read from replicas. Writable
continuation (shipping a new SPA version) needs a primary and runs into the
same gaps as the user-data bucket: new agreement means new bucket, and the
data reaches the new provider only through client download and upload. The
operator has the SPA build locally, so re-upload is possible but changes the
`bucket_id` that DNS and users point at.

**Q3. Can the transfer run provider-to-provider instead of through the
client?** Replica sync already moves a whole bucket between two providers
with every node verified against the chain root. Nothing uses it for a
primary. #65 asks the same question from the provider side (§1
primary-to-primary, §2 primary-to-replica, §4 provider migration) and has no
protocol.

## Designed vs. implemented

| Topic | Design | Code |
|---|---|---|
| Several primaries per bucket | Yes. "Added only by bucket admin", up to ~5 (design doc, "Buckets: Stable Identity"). Use cases start with "Add 2-3 diverse providers". | Shape exists: `primary_providers` (`MaxPrimaryProviders` = 5), N-signature `checkpoint`, positional signer bitfield, removal on expiry. |
| Adding a primary to an existing bucket | **Not specified.** The only primary flow is `establish_storage_agreement`, which creates the bucket. | **Missing.** `establish_storage_agreement` rejects `terms.bucket_id = Some(_)` and always creates a bucket (`impls/agreements.rs:151`, `:218`). Tests and benchmarks write the multi-primary shape directly into storage (`mock.rs:430`, `benchmarking.rs:214`). |
| Moving a bucket to another provider | **Not specified.** | **Missing.** |
| Data movement between primaries | "Primary providers don't sync with each other. Clients are responsible for uploading to each primary" (impl doc, "Multi-Provider Coordination", line 110). A lagging primary syncs by "client re-uploads" (line 116). | None. |
| Replica sync | Autonomous, from primaries or other replicas; `confirm_replica_sync`; challengeable. Read-only. | Implemented (`provider-node/src/replica_sync*.rs`). Syncs only `Replica`-role agreements (`subxt_client.rs:527`), fetches only from primaries. Endpoints `/mmr_peaks`, `/mmr_subtree`, `/fetch_nodes` are role-agnostic. |
| Liability of a primary | Only for snapshots it signed. | `challenge_checkpoint` requires the signer bit (`lib.rs:2230`). `extend_checkpoint` needs a writer/admin origin and drops a bit beyond the stored bitfield silently (`lib.rs:2180-2182`). |
| Continuation on one provider | `extend_agreement` before expiry. Binding to expiry, no auto-renew. | Implemented. Rejects expired agreements (`lib.rs:1942`). |
| After expiry | "Provider is no longer bound to store data." No retention window. Also: "liable for snapshots they signed until superseded". | All three challenge extrinsics require `anchor < expires_at`. Liability ends at the expiry block. `SettlementTimeout` (24h) is a payment window. |
| `transfer_agreement_ownership` | Changes the owner account of one agreement. Same provider, same bucket. | PR #414 (open) implements it. Moves the payer, not the provider or the data. |
| Same provider in two roles | — | Agreements keyed by `(bucket, provider)`. A provider cannot be replica and primary of one bucket at once. |
| Private buckets | Primaries serve reads only to members; provider accounts are not members. | Read and sync endpoints are unauthenticated (#383). |
| Layer 1 | — | `DriveInfo.expires_at` / `max_capacity` are write-once copies of the first terms. `delete_drive` → `cleanup_bucket_internal` ends every agreement early with pro-rated refunds, replicas included, no role check. |

## Gaps

Transfer:

- **G1** No way to add a primary to an existing bucket, and no way to move a
  bucket to another provider. Also the reason a bucket whose sole primary
  expired, was slashed or was early-terminated can never be written again:
  `checkpoint` has no eligible signer.
- **G2** No primary-to-primary data movement. The design assigns it to the
  client, which the story cannot do.
- **G3** No way for a primary to attest possession on its own. Only a
  writer/admin can add its signature, and the bitfield is sized at
  checkpoint time.
- **G4** Private bucket: a joining provider is not a member, so it has no
  honest source unless the admin adds it as `Reader` or a replica exists.
- **G5** A provider cannot be replica and primary of one bucket at once, so
  "keep a replica as warm standby, promote it later" has a liability gap
  between the two agreements.
- **G6** Layer 1 mirrors `expires_at` / `max_capacity` from the first terms
  and has no update path.

Continuity:

- **C1** Paid is not liable. A second primary that never signs is paid and
  never slashable. `min_providers = 2` forces both signatures but one dead
  primary then blocks all checkpoints until the admin lowers it or
  early-terminates (full payment). No intermediate setting.
- **C2** Renewal needs a live, funded owner before the expiry block. No
  auto-renew, no prepaid pool. Missing the block on the sole primary
  triggers G1.
- **C3** No notice obligation: a provider can block extensions one block
  before expiry.
- **C4** Just-in-time migration depends on the leaving provider serving the
  fetch, which is unpaid (read incentives not implemented).
- **C5** `delete_drive` ends every agreement on the bucket early with
  refunds, including third-party replicas. Contradicts "no early
  cancellation for clients" and "replicas cannot be early-terminated".
- **C6** Replica attestation must match the current root or one of six
  historical roots (oldest ~113 anchor blocks). A large, frequently
  checkpointed bucket may never let a replica confirm.
- **C7** Nobody challenges automatically. An offline provider stays paid
  until a human challenges, then 48h pass before `remove_slashed`.

Retention:

- **R1** No window after `expires_at`. The provider may delete at the expiry
  block. Any transfer must complete before it. `SettlementTimeout` is for
  payment only.

Economics (inherent, to document):

- **E1** Overlap payment to `P1` and `P2` for the transfer window.
- **E2** Serving the bulk fetch is unpaid; the operator's authorized-tier
  challenge against `P1` is the only lever.

## Drifts (design vs. code)

1. The design's use cases assume a join path ("Add 2-3 diverse providers")
   that the implementation doc never specifies and the pallet does not have.
2. `extend_checkpoint` drops a signer bit beyond the stored bitfield and still
   emits `BucketCheckpointed` for that provider (`lib.rs:2180-2182`).
3. "Snapshot liability remains until superseded" does not apply after expiry
   in code.
4. `delete_drive` early-terminates replica agreements with refunds (C5).
5. The provider node accepts `PUT /node` and `POST /commit` for any bucket a
   writer authenticates for, without checking its own agreement or role
   (`provider-node/src/api.rs:262-303`). A replica accepts writes it can never
   checkpoint. Quota part is #382; the role check is unfiled.
6. Read and sync endpoints are unauthenticated while the design gates private
   bucket reads to members (#383).
7. `transfer_agreement_ownership` was specified and unimplemented
   (DRIFT-015 in #376); PR #414 closes it.

## Questions for the design owner

Ownership and shape:

1. Is a bucket-level provider change intended: move the bucket to a new
   provider under a new agreement, add a primary to an existing bucket, or
   both? The design promises a stable `bucket_id` across providers and gives
   no path to keep it.
2. Which ownership shape does the design intend for dApp user data:
   operator-owned shared bucket, per-user buckets, or contract-owned buckets
   (Q1)? Should the design doc add a "dApp with user data" use case?
3. After #414 a drive owner and its agreement owner can diverge. Does Layer 1
   need a matching drive transfer?

Data movement and liability:

4. Should "Primary providers don't sync with each other" be amended so a
   primary may catch up from any provider with a live agreement on the
   bucket, with clients still responsible for delivering new writes to every
   primary?
5. Should a primary be able to attest the current snapshot itself, so its
   liability does not depend on a live client relaying its signature?
6. Should a provider be able to be replica and primary of one bucket at once,
   or be promoted from replica to primary in place?
7. Is there a setting between "one signature is enough" and "all primaries
   must sign" that keeps a second primary liable without blocking
   checkpoints when one is dead?

Lifecycle:

8. Should a `RetentionPeriod` after `expires_at` exist, during which the
   provider stays challengeable and the agreement cannot be settled?
9. Should a provider be required to announce a refused extension a minimum
   number of blocks before expiry?
10. Is a pro-rated early exit ever acceptable (for example once a replacement
    provider has attested), or does the binding-commitment rule stay
    absolute? `delete_drive` already does this today (C5).
11. Layer 1: derive mirrored agreement fields from Layer 0, or add update
    paths?

## Related

- Migration and wind-down: #65 (§1, §2, §4), #281, #107.
- Multi-provider checkpoints: `docs/drafts/CHECKPOINT_PROTOCOL.md`, #332,
  #388, #302.
- Multi-writer and dApp buckets: #134, #133, `docs/drafts/smart-contracts.md`,
  design doc "Media in Chat" and "Public Website" use cases. No dedicated
  multi-writer issue exists.
- Ownership transfer: #414, #376 (DRIFT-015), #403.
- Provider-node gaps: #382, #383, #310.
- `docs/drafts/marketplace.md`, "Automatic Data Migration".
