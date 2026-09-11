# Potential RFC: Bucket transfer between providers (Draft)

> **Draft — needs triage.** Gap summary for discussion with the design owner.
> No proposal. Written against `dev` at 67ce0738 (2026-09-11).

## Story

A dApp operator owns bucket `B`. The dApp's users write into `B`. The operator
never had a local copy of the data and cannot ask the users to upload it
again.

`B` has one primary provider `P1` under a 1-year agreement. Before it expires,
`B` must continue on provider `P2`:

- same `bucket_id`, because DNS TXT / DotNS records, `bucket://` URIs, Layer 1
  drives, replica agreements, members and contracts all reference it, and the
  design promises "the bucket_id never changes";
- `P2` is a primary, because replicas are read-only and the dApp keeps
  writing;
- `P2` gets the data from `P1` or a replica, nobody re-uploads;
- the operator sees on chain that `P2` has the data before `P1` leaves.

**Today this is not possible.** The only way to put a second provider under
contract is `establish_storage_agreement`, which creates a new bucket. The
design describes multi-primary buckets ("Add 2-3 diverse providers") but
specifies no way to add a primary to an existing bucket, and the pallet has
none. Data movement between primaries is assigned to the client ("client
re-uploads"). Replicas sync automatically but cannot take writes.

## Main questions

1. **Bucket-level provider change.** Is it intended that a bucket moves to a
   new provider, or that a primary is added to an existing bucket? The
   design promises a stable `bucket_id` across providers and has no path to
   keep it.
2. **Provider-to-provider transfer.** Should a joining primary obtain the
   data from any provider under agreement on the bucket, the way replicas do,
   instead of the client re-uploading? (#65 §1, §4)
3. **Liability of a joining primary.** A primary is slashable only for
   snapshots it signed, and only a writer/admin can add its signature. Should
   a primary attest possession itself? Is there a middle ground between
   "one signature is enough" and "all primaries must sign"?
4. **Replica to primary.** Should a provider be replica and primary of one
   bucket at once, or be promoted in place, so a replica can serve as warm
   standby?
5. **Retention after expiry.** The provider may delete data at the expiry
   block; all challenges stop there. Should a retention period exist during
   which the provider stays challengeable, so a transfer has a window?
6. **Renewal and notice.** Renewal needs a live, funded owner before the
   expiry block, and a provider can block extensions one block before it.
   Should there be auto-renew from escrow and a minimum notice period?
7. **dApp user data ownership.** One operator bucket (users as writers, a
   shared key, or a contract), one bucket per user, or contract-owned
   buckets? Does every user need an agreement? The design has no dApp use
   case.
8. **Layer 1.** `delete_drive` ends every agreement on the bucket early with
   refunds, including third-party replicas. Drive fields mirror the first
   agreement and never update. After #414 a drive owner and its agreement
   owner can diverge. What does Layer 1 want here?

## Gaps

- No extrinsic adds a primary to an existing bucket or moves a bucket. A
  bucket whose sole primary expired or was slashed can never be written
  again.
- No primary-to-primary data movement. Replica sync has the mechanism; it
  runs only for the replica role and fetches only from primaries.
- A primary cannot attest possession on its own. A second primary that
  never signs is paid and never slashable.
- A provider cannot be replica and primary of one bucket at once.
- On a private bucket a joining provider has no honest data source unless
  the admin adds it as a member or a replica exists.
- No retention window after expiry. No auto-renew. No notice obligation.
- Replica confirmation must match one of the last ~113 anchor blocks' roots;
  a large busy bucket may never let a replica confirm.
- Nobody challenges automatically.
- Overlap payment and unpaid bulk fetch are inherent to the current
  economics.

## Drifts (design vs. code)

- Use cases assume adding providers to a bucket; nothing specifies or
  implements it.
- `extend_checkpoint` silently drops a signer bit beyond the stored bitfield.
- "Liable for signed snapshots until superseded" ends at expiry in code.
- `delete_drive` early-terminates replicas with refunds; the design forbids
  both.
- The provider node accepts uploads and commits for buckets where it has no
  agreement or the wrong role (#382 covers quota).
- Read and sync endpoints are unauthenticated (#383).
- `transfer_agreement_ownership` was specified and missing; PR #414 adds it.
  It moves the payer, not the data.

## Related

#65, #281, #107 (migration, wind-down) · #332, #388, #302 (multi-provider
checkpoints) · #134, #133 (dApps) · #414, #376 (ownership transfer) · #382,
#383, #310 (provider node) · `docs/drafts/CHECKPOINT_PROTOCOL.md`,
`docs/drafts/smart-contracts.md`, `docs/drafts/marketplace.md`.
