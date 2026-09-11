# Bucket transfer between providers: design gaps (Draft)

> **Draft — needs triage.** Gap summary for discussion with the design owner.
> No proposal. Written against `dev` at 67ce0738 (2026-09-11). Design changes
> go through a `docs/design/` PR per `.github/CODEOWNERS`.

## Story

A dApp operator owns bucket `B`. The dApp's users write into `B` (as `Writer`
members, through a shared writer key, or through a `pallet_revive` contract).
The operator never had a local copy of the data and cannot ask the users to
upload it again.

`B` has one primary provider `P1` under a 1-year agreement. Before it expires,
`B` must continue on provider `P2` with the same `bucket_id`, `P2` must obtain
the data from `P1` or a replica without anyone re-uploading, and the operator
needs on-chain evidence that `P2` has the data before `P1` leaves. `P2` must
be a primary: replicas are read-only and the dApp keeps writing.

Other triggers for the same story: `P1` winds down (#281), blocks extensions,
raises prices, or is slashed. The SPA bucket itself has the same problem
whenever the operator wants to ship a new version after a provider change.

### Why `bucket_id` must not change

The design makes `bucket_id` the stable reference: "The bucket_id is the
stable anchor. Providers can come and go, but the bucket_id never changes.
Applications reference buckets, not providers." (design doc, "Bucket
Addressing"). Depending on it: `bucket://<id>/...` URIs, DNS TXT and DotNS
records (design doc, "Public Website"), `DriveInfo.bucket_id` and the S3
mapping, replica agreements keyed by `bucket_id`, members, `min_providers`,
`frozen_start_seq`, visibility, the snapshot history, and contracts that
store the id.

Today the only way to put a second provider under contract is
`establish_storage_agreement`, which creates a new bucket. "New agreement with
`P2`" therefore means a new `bucket_id`, a full re-upload by whoever has the
data, re-creating members and settings, re-funding replicas, and updating
every reference above.

**Net: the story is not possible today.** `P2` can only get a new bucket, and
only the users' clients could fill it. The story needs a bucket-level provider
change: move the bucket to another provider under a new agreement, or at
least add a primary provider to an existing bucket. The design specifies
neither.

## Gaps

Transfer:

- **G1** No way to add a primary to an existing bucket, and no way to move a
  bucket to another provider. Also the reason a bucket whose sole primary
  expired, was slashed or was early-terminated can never be written again.
- **G2** No primary-to-primary data movement. The design assigns it to the
  client, which the story cannot do. Replica sync has the mechanism; nothing
  uses it for a primary. #65 §1 and §4 ask for the same from the provider
  side.
- **G3** A primary cannot attest possession on its own. Only a writer/admin
  can add its signature, and the bitfield is sized at checkpoint time.
- **G4** Private bucket: a joining provider is not a member and has no honest
  source unless the admin adds it as `Reader` or a replica exists.
- **G5** A provider cannot be replica and primary of one bucket at once, so
  "keep a replica as warm standby, promote it later" has a liability gap.
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
  fetch, which is unpaid.
- **C5** `delete_drive` ends every agreement on the bucket early with
  refunds, including third-party replicas. Contradicts "no early
  cancellation for clients" and "replicas cannot be early-terminated".
- **C6** Replica attestation must match the current root or one of six
  historical roots (oldest ~113 anchor blocks). A large, frequently
  checkpointed bucket may never let a replica confirm.
- **C7** Nobody challenges automatically. An offline provider stays paid
  until a human challenges, then 48h pass before `remove_slashed`.

Retention and economics:

- **R1** No window after `expires_at`. The provider may delete at the expiry
  block. Any transfer must complete before it.
- **E1** Overlap payment to `P1` and `P2` for the transfer window (inherent).
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
   (`provider-node/src/api.rs:262-303`). Quota part is #382; the role check
   is unfiled.
6. Read and sync endpoints are unauthenticated while the design gates private
   bucket reads to members (#383).
7. `transfer_agreement_ownership` was specified and unimplemented
   (DRIFT-015 in #376); PR #414 closes it.

## Questions

Ownership and shape:

1. Is a bucket-level provider change intended: move the bucket to a new
   provider under a new agreement, add a primary to an existing bucket, or
   both? The design promises a stable `bucket_id` across providers and has no
   path to keep it.
2. Who owns and pays for dApp user data? Options, none specified: one
   operator bucket with users as `Writer` members (bounded by `MaxMembers`);
   one operator bucket with a shared writer key or a contract as writer;
   one bucket per user with a user-owned agreement (Web3 principle: the user
   pays and controls, but every user needs funds and signatures, and the
   continuity problem moves to every user); contract-owned per-user buckets.
   Does every user need an agreement? Should the design add a "dApp with user
   data" use case?
3. How does a static SPA bucket continue past `expires_at`? Read-only
   continuation works today through replicas of a frozen public bucket.
   Writable continuation (new SPA version) needs a primary and hits G1 and
   G2; the operator has the build locally, but re-upload changes the
   `bucket_id` that DNS points at.
4. After #414 a drive owner and its agreement owner can diverge. Does Layer 1
   need a matching drive transfer?

Data movement and liability:

5. Can the transfer run provider-to-provider instead of through the client?
   Should "Primary providers don't sync with each other" be amended so a
   primary may catch up from any provider with a live agreement on the
   bucket, with clients still delivering new writes to every primary?
6. Should a primary be able to attest the current snapshot itself, so its
   liability does not depend on a client relaying its signature?
7. Should a provider be able to be replica and primary of one bucket at once,
   or be promoted from replica to primary in place?
8. Is there a setting between "one signature is enough" and "all primaries
   must sign" that keeps a second primary liable without blocking
   checkpoints when one is dead?

Lifecycle:

9. Should a `RetentionPeriod` after `expires_at` exist, during which the
   provider stays challengeable and the agreement cannot be settled?
10. Should a provider be required to announce a refused extension a minimum
    number of blocks before expiry?
11. Is a pro-rated early exit ever acceptable (for example once a replacement
    provider has attested), or does the binding-commitment rule stay
    absolute? `delete_drive` already does this today (C5).
12. Layer 1: derive mirrored agreement fields from Layer 0, or add update
    paths?

## Designed vs. implemented

| Topic | Design | Code |
|---|---|---|
| Several primaries per bucket | Yes. "Added only by bucket admin", up to ~5 (design doc, "Buckets: Stable Identity"). Use cases start with "Add 2-3 diverse providers". | Shape exists: `primary_providers` (`MaxPrimaryProviders` = 5), N-signature `checkpoint`, positional signer bitfield, removal on expiry. |
| Adding a primary to an existing bucket | **Not specified.** The only primary flow is `establish_storage_agreement`, which creates the bucket. | **Missing.** `establish_storage_agreement` rejects `terms.bucket_id = Some(_)` and always creates a bucket (`impls/agreements.rs:151`, `:218`). Tests and benchmarks write the multi-primary shape directly into storage. |
| Moving a bucket to another provider | **Not specified.** | **Missing.** |
| Data movement between primaries | "Primary providers don't sync with each other. Clients are responsible for uploading to each primary" (impl doc, "Multi-Provider Coordination", line 110). A lagging primary syncs by "client re-uploads" (line 116). | None. |
| Replica sync | Autonomous, from primaries or other replicas; `confirm_replica_sync`; challengeable. Read-only. | Implemented (`provider-node/src/replica_sync*.rs`). Syncs only `Replica`-role agreements (`subxt_client.rs:527`), fetches only from primaries. Endpoints `/mmr_peaks`, `/mmr_subtree`, `/fetch_nodes` are role-agnostic. |
| Liability of a primary | Only for snapshots it signed. | `challenge_checkpoint` requires the signer bit (`lib.rs:2230`). `extend_checkpoint` needs a writer/admin origin and drops a bit beyond the stored bitfield silently (`lib.rs:2180-2182`). |
| Continuation on one provider | `extend_agreement` before expiry. Binding to expiry, no auto-renew. | Implemented. Rejects expired agreements (`lib.rs:1942`). |
| After expiry | "Provider is no longer bound to store data." No retention window. Also: "liable for snapshots they signed until superseded". | All three challenge extrinsics require `anchor < expires_at`. Liability ends at the expiry block. `SettlementTimeout` (24h) is a payment window. |
| `transfer_agreement_ownership` | Changes the owner account of one agreement. Same provider, same bucket. | PR #414 (open) implements it. Moves the payer, not the provider or the data. |
| Same provider in two roles | — | Agreements keyed by `(bucket, provider)`. A provider cannot be replica and primary of one bucket at once. |
| Private buckets | Primaries serve reads only to members; provider accounts are not members. | Read and sync endpoints are unauthenticated (#383). |
| dApp user data ownership | Not covered. Closest use cases: "Media in Chat" (shared bucket, member writers), "Public Website" (static, frozen). | `MaxMembers` = 100, `MaxBucketsPerMember` = 1000; contract write path in `docs/drafts/smart-contracts.md`. |
| Layer 1 | — | `DriveInfo.expires_at` / `max_capacity` are write-once copies of the first terms. `delete_drive` → `cleanup_bucket_internal` ends every agreement early with pro-rated refunds, replicas included, no role check. |

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
