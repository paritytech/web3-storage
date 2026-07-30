# Storage Provider Pallet - Extrinsics Reference

Quick reference for all available extrinsics in the storage provider pallet ([crates/pallets/storage-provider/src/lib.rs](../../crates/pallets/storage-provider/src/lib.rs)).

## Provider Management

### `registerProvider`

Register a new storage provider.

**Parameters:**
- `multiaddr`: `BoundedVec<u8, T::MaxMultiaddrLength>` - network address (e.g. `/ip4/127.0.0.1/tcp/3333`)
- `publicKey`: `BoundedVec<u8, ConstU32<64>>` - raw public key (32 bytes for Sr25519/Ed25519, 33 bytes for ECDSA)
- `stake`: `BalanceOf<T>` - Amount to stake (must be ≥ `MinProviderStake`)

**Example:**
```
multiaddr: /ip4/127.0.0.1/tcp/3333
publicKey: 0xd43593c715fdd31c61141abd04a99fd6822c8558854ccde39a5684e7a56da27d
stake: 1000000000000000  (1000 tokens, minimum required)
```

**Token decimals:**
- 1 token = 1,000,000,000,000 (12 decimals, like Polkadot)
- Minimum stake = 1000 tokens = 1,000,000,000,000,000

**Default settings after registration** — all zero / `false`. You must call `updateProviderSettings` before any agreement can be accepted.

**Events:** `ProviderRegistered`
**Errors:** `ProviderAlreadyRegistered`, `InsufficientStake`, `InvalidPublicKey`

---

### `addStake`

Increase the provider's locked stake.

**Parameters:**
- `amount`: `BalanceOf<T>` - additional amount to reserve

**Example:**
```
amount: 50000000000
```

**Events:** `ProviderStakeAdded`
**Errors:** `ProviderNotFound`, `ArithmeticOverflow`

---

### `deregisterProvider`

**Step 1 of a two-step exit.** Announces deregistration: forces `acceptingPrimary` and `acceptingExtensions` to `false`, stamps `deregister_at = now + DeregisterAnnouncementPeriod`. Stake stays reserved and the provider remains slashable. Settings updates are blocked during the announcement window.

**Parameters:** none

**Example:**
```
// no parameters — caller is the provider account
deregisterProvider()
```

**Preconditions:** `committed_bytes == 0` (no active agreements); no existing announcement.

**Events:** `DeregisterAnnounced { complete_after }`
**Errors:** `ProviderNotFound`, `DeregisterAnnounced`, `ProviderHasActiveAgreements`

---

### `updateProviderSettings`

Update pricing, availability, and capacity.

**Parameters:**
- `settings`: `ProviderSettings<T>`
  - `minDuration`: `BlockNumber` - minimum agreement duration
  - `maxDuration`: `BlockNumber` - maximum agreement duration
  - `pricePerByte`: `Balance` - price per byte per block
  - `acceptingPrimary`: `bool` - accept new primary agreements
  - `replicaSyncPrice`: `Option<Balance>` - `None` disables replicas; `Some(price)` enables them at that per-sync price
  - `acceptingExtensions`: `bool` - accept agreement extensions
  - `maxCapacity`: `u64` - maximum storage capacity in bytes (`0` = unlimited)

**Example:**
```
settings: {
  minDuration: 100,
  maxDuration: 10000,
  pricePerByte: 1000000,
  acceptingPrimary: true,
  replicaSyncPrice: Some(5000000),
  acceptingExtensions: true,
  maxCapacity: 1099511627776  // 1 TB
}
```

**Validation:**
- `minDuration ≤ maxDuration`
- If `maxCapacity > 0`: `maxCapacity ≥ committed_bytes` and `stake ≥ maxCapacity × MinStakePerByte`
- Blocked while a deregistration announcement is pending

**Events:** `ProviderSettingsUpdated`
**Errors:** `ProviderNotFound`, `DeregisterAnnounced`, `MinDurationExceedsMaxDuration`, `CapacityBelowCommitted`, `InsufficientStakeForCapacity`

---

### `setExtensionsBlocked`

Per-bucket toggle that lets a provider refuse `extendAgreement` calls for one specific agreement without changing their global `acceptingExtensions` flag.

**Parameters:**
- `bucketId`: `BucketId` (u64)
- `blocked`: `bool`

**Example:**
```
bucketId: 0
blocked: true
```

**Events:** `ExtensionsBlocked`
**Errors:** `ProviderNotFound`, `AgreementNotFound`, `AgreementExpired`

---

### `updateProviderMultiaddr`

Update only the provider's network endpoint.

**Parameters:**
- `multiaddr`: `BoundedVec<u8, T::MaxMultiaddrLength>` - new network address

**Example:**
```
multiaddr: /ip4/203.0.113.10/tcp/3333
```

**Events:** `ProviderMultiaddrUpdated`
**Errors:** `ProviderNotFound`

---

### `completeDeregister`

**Step 2 of the two-step exit.** Unreserves stake and removes the provider record.

**Parameters:** none

**Example:**
```
// no parameters — caller is the provider account
completeDeregister()
```

**Preconditions:** previously announced (`deregister_at` set) and `now ≥ deregister_at`; still `committed_bytes == 0`.

**Events:** `ProviderDeregistered { stake_returned }`
**Errors:** `ProviderNotFound`, `DeregisterNotAnnounced`, `DeregisterPeriodNotElapsed`, `ProviderHasActiveAgreements`

---

### `cancelDeregister`

Cancel a pending announcement. Clears `deregister_at` and restores `acceptingPrimary` / `acceptingExtensions` to `true`.

**Parameters:** none

**Example:**
```
// no parameters — caller is the provider account
cancelDeregister()
```

**Events:** `DeregisterCancelled`
**Errors:** `ProviderNotFound`, `DeregisterNotAnnounced`

---

## Bucket Management

### `createBucket`

Create an empty bucket; caller becomes its sole admin.

**Parameters:**
- `minProviders`: `u32` - minimum primary-provider signatures required for a valid checkpoint
- `visibility`: `Public | Private` - whether primaries serve reads to anyone or only to members (cooperative, not on-chain-enforced; full semantics in the design doc's "Bucket Visibility & Access"). Wrappers omitting the choice must default to `Private` (fail-safe).

**Example:**
```
minProviders: 2
visibility: Private
```

**Returns:** Emits `BucketCreated` event with assigned `bucketId`.

**Events:** `BucketCreated { bucket_id, admin }`
**Errors:** `MaxMembersReached`, `TooManyBucketsForMember`

---

### `createBucketWithStorage`

One-shot helper: create a bucket, find the cheapest matching provider, and accept the primary agreement, all atomically.

**Parameters:**
- `maxBytes`: `u64` - desired storage size
- `duration`: `BlockNumber` - agreement duration
- `maxPricePerByte`: `Balance` - cap on provider's `pricePerByte`
- `visibility`: `Public | Private` - see `createBucket`

**Example:**
```
maxBytes: 1073741824   // 1 GB
duration: 500
maxPricePerByte: 1000000
visibility: Private
```

**Events:** `BucketCreated`, `AgreementAccepted`, `ProviderAddedToBucket`
**Errors:** `NoMatchingProvider`, `PaymentExceedsMax`, `InsufficientStakeForBytes`, `CapacityExceeded`, `MaxMembersReached`, `TooManyBucketsForMember`

---

### `setMinProviders`

Change the checkpoint signature threshold for an existing bucket.

**Parameters:**
- `bucketId`: `BucketId` (u64)
- `minProviders`: `u32` - must not exceed the current primary-provider count

**Example:**
```
bucketId: 0
minProviders: 3
```

**Errors:** `BucketNotFound`, `NotBucketAdmin`, `InvalidMinProviders`

---

### `freezeBucket`

Freeze a bucket at the current snapshot's `start_seq`. Append-only afterwards; irreversible.

**Parameters:**
- `bucketId`: `BucketId` (u64)

**Example:**
```
bucketId: 0
```

⚠️ Requires a snapshot with at least `minProviders` signatures.

**Events:** `BucketFrozen { frozen_start_seq }`
**Errors:** `BucketNotFound`, `NotBucketAdmin`, `BucketFrozen`, `NoSnapshot`, `MinProvidersNotMet`

---

### `setBucketVisibility`

Flip a bucket between `Public` and `Private` (admin only). Always allowed in both directions; effects are asymmetric—privatizing does not recall data already replicated, publicizing cannot be undone. See "Transitions" in the design doc's Bucket Visibility & Access section.

**Parameters:**
- `bucketId`: `BucketId` (u64)
- `visibility`: `Public | Private`

**Example:**
```
bucketId: 0
visibility: Public
```

**Events:** `BucketVisibilityChanged`
**Errors:** `BucketNotFound`, `NotBucketAdmin`

---

### `setMember`

Add a member or update an existing member's role.

**Parameters:**
- `bucketId`: `BucketId` (u64)
- `member`: `AccountId` - account to add or update
- `role`: `Role` - one of `Admin`, `Writer`, `Reader`. `Admin` and `Writer` can read; `Reader` grants read-only access (meaningful on private buckets, where membership is the read access list). Adding a Reader does not by itself make a bucket private—use `setBucketVisibility`.

**Example:**
```
bucketId: 0
member: 5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty
role: Writer
```

**Admin-quorum rules:**
- Caller must be an admin.
- An admin can only **demote themselves**, never another admin → `CannotDemoteAdmin`.
- Self-demotion is rejected if it would leave the bucket with zero admins → `LastAdminCannotBeRemoved`.

**Events:** `MemberSet`
**Errors:** `BucketNotFound`, `NotBucketAdmin`, `CannotDemoteAdmin`, `LastAdminCannotBeRemoved`, `MaxMembersReached`, `TooManyBucketsForMember`

---

### `removeMember`

Remove a member from a bucket. Same admin-quorum rules as `setMember`: admins can only remove themselves and never the last admin.

**Parameters:**
- `bucketId`: `BucketId` (u64)
- `member`: `AccountId`

**Example:**
```
bucketId: 0
member: 5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty
```

**Events:** `MemberRemoved`
**Errors:** `BucketNotFound`, `NotBucketAdmin`, `MemberNotFound`, `CannotDemoteAdmin`, `LastAdminCannotBeRemoved`

---

### `removeSlashed`

**Permissionless cleanup.** Anyone can call this to evict a slashed provider (one with `stake == 0`) from a bucket. The agreement's locked payment is returned to its owner; if the provider was a primary, it's removed from the bucket's primary list.

**Parameters:**
- `bucketId`: `BucketId` (u64)
- `provider`: `AccountId`

**Example:**
```
bucketId: 0
provider: 5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY
```

**Events:** `SlashedProviderRemoved`, plus `PrimaryProviderRemoved { reason: Slashed }` if applicable
**Errors:** `ProviderNotFound`, `ProviderNotSlashed`, `AgreementNotFound`

---

## Agreement Management

### `requestAgreement`

Request a **replica** agreement (provider must have `replicaSyncPrice: Some(_)`).

**Parameters:**
- `bucketId`: `BucketId` (u64)
- `provider`: `AccountId` - replica provider
- `maxBytes`: `u64` - storage size
- `duration`: `BlockNumber` - agreement duration
- `maxPayment`: `Balance` - safety cap; must cover `pricePerByte × maxBytes × duration`
- `replicaParams`: `ReplicaRequestParams<Balance, BlockNumber>`
  - `syncBalance`: `Balance` - funds reserved up-front for future sync payments
  - `minSyncInterval`: `BlockNumber` - rate-limit between confirmed syncs

**Example:**
```
bucketId: 0
provider: 5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty
maxBytes: 1073741824   // 1 GB
duration: 500
maxPayment: 600000000000000000
replicaParams: {
  syncBalance: 50000000,
  minSyncInterval: 100
  syncPrice: 1000000,
}
```

Funds reserved on request: storage payment **plus** `syncBalance`. The request expires if not accepted within `RequestTimeout`.

**No syncability check:** a private bucket with no replicas is still accepted—an unfulfillable agreement is the funder's own risk. Rationale: design doc, "No on-chain gate on replica creation".

**Events:** `AgreementRequested`
**Errors:** `BucketNotFound`, `ProviderNotFound`, `DeregisterAnnounced`, `ProviderNotAcceptingReplicas`, `DurationTooShort`, `DurationTooLong`, `PaymentExceedsMax`, `AgreementRequestAlreadyExists`

---

### `requestPrimaryAgreement`

Request a **primary** storage agreement. Caller must be a bucket admin.

**Parameters:**
- `bucketId`: `BucketId` (u64)
- `provider`: `AccountId`
- `maxBytes`: `u64` - maximum storage size
- `duration`: `BlockNumber` - agreement duration in anchor (relay-chain) blocks
- `maxPayment`: `Balance` - maximum payment willing to pay (safety cap)

**Example:**
```
bucketId: 0
provider: 5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY
maxBytes: 1073741824   // 1 GB
duration: 500
maxPayment: 600000000000000000
```

**Payment calculation:**
```
payment = pricePerByte × maxBytes × duration
payment = 1,000,000 × 1,073,741,824 × 500
payment = 536,870,912,000,000,000
```

**Important:** `maxPayment` must be ≥ calculated payment, or you'll get `PaymentExceedsMax`. Add a 10-20% buffer. See [Payment Calculator](./PAYMENT_CALCULATOR.md).

**Events:** `AgreementRequested`
**Errors:** `BucketNotFound`, `NotBucketAdmin`, `MaxPrimaryProvidersReached`, `ProviderNotFound`, `DeregisterAnnounced`, `ProviderNotAcceptingPrimary`, `DurationTooShort`, `DurationTooLong`, `PaymentExceedsMax`, `AgreementRequestAlreadyExists`

---

### `acceptAgreement`

Provider accepts a pending agreement request. Creates the `StorageAgreement`, starts the clock, and (for primaries) adds the provider to the bucket's primary list.

**Parameters:**
- `bucketId`: `BucketId` (u64)

**Example:**
```
bucketId: 0
```

**Note:** Caller must be the provider specified in the request. Validates request hasn't expired (`RequestTimeout`) and that the provider has capacity headroom and enough stake (`stake ≥ committed_bytes × MinStakePerByte`).

**Events:** `AgreementAccepted`, plus `ProviderAddedToBucket` for primaries
**Errors:** `ProviderNotFound`, `DeregisterAnnounced`, `AgreementRequestNotFound`, `RequestExpired`, `InsufficientStakeForBytes`, `CapacityExceeded`, `MaxPrimaryProvidersReached`

---

### `rejectAgreement`

Provider rejects a pending request; all reserved funds (payment + sync balance) return to the requester.

**Parameters:**
- `bucketId`: `BucketId` (u64)

**Example:**
```
bucketId: 0
```

**Events:** `AgreementRejected`
**Errors:** `AgreementRequestNotFound`

---

### `withdrawAgreementRequest`

Original requester rescinds a pending request before acceptance.

**Parameters:**
- `bucketId`: `BucketId` (u64)
- `provider`: `AccountId`

**Example:**
```
bucketId: 0
provider: 5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY
```

**Events:** `AgreementRequestWithdrawn`
**Errors:** `AgreementRequestNotFound`, `NotAgreementOwner`

---

### `extendAgreement`

Extend an active agreement. Settles elapsed time at the **old** rate, then locks new payment at the provider's **current** rate for the additional duration.

**Parameters:**
- `bucketId`: `BucketId` (u64)
- `provider`: `AccountId`
- `additionalDuration`: `BlockNumber`
- `maxPayment`: `Balance` - cap on payment for the extension

**Example:**
```
bucketId: 0
provider: 5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY
additionalDuration: 500
maxPayment: 600000000000000000
```

**Authorization:**
- If the provider has **raised** their price, only the agreement owner can extend.
- If the price is the **same or lower**, anyone can extend (permissionless persistence — useful for frozen buckets).

**Events:** `AgreementExtended`
**Errors:** `ProviderNotFound`, `DeregisterAnnounced`, `ProviderNotAcceptingExtensions`, `AgreementNotFound`, `AgreementExpired`, `AgreementExtensionsBlocked`, `DurationTooShort`, `DurationTooLong`, `PaymentExceedsMax`, `NotAgreementOwner`

---

### `topUpAgreement`

Owner increases the agreement's byte quota mid-term. Charges for the additional bytes over the remaining duration at the provider's current price.

**Parameters:**
- `bucketId`: `BucketId` (u64)
- `provider`: `AccountId`
- `additionalBytes`: `u64`
- `maxPayment`: `Balance` - cap on payment for the additional bytes

**Example:**
```
bucketId: 0
provider: 5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY
additionalBytes: 536870912   // 0.5 GB
maxPayment: 300000000000000000
```

**Events:** `AgreementToppedUp`
**Errors:** `ProviderNotFound`, `DeregisterAnnounced`, `AgreementNotFound`, `NotAgreementOwner`, `AgreementExpired`, `PaymentExceedsMax`

---

### `endAgreement`

End an agreement, either early or after expiry.

**Parameters:**
- `bucketId`: `BucketId` (u64)
- `provider`: `AccountId`
- `action`: `EndAction` - `Pay` (release locked payment to provider) or `Burn`

**Example:**
```
bucketId: 0
provider: 5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY
action: Pay
```

**Authorization:**
- Early termination: only a bucket admin, and only for **primary** agreements (replicas can't be terminated mid-flight).
- After expiry: only the agreement owner, within the `SettlementTimeout` window.

**Events:** `AgreementEnded`, plus `PrimaryProviderRemoved` if applicable
**Errors:** `AgreementNotFound`, `NotBucketAdmin`, `NotAgreementOwner`, `CannotTerminateReplica`, `SettlementWindowPassed`

---

### `claimExpiredAgreement`

After the settlement window passes, the provider can claim the locked payment themselves.

**Parameters:**
- `bucketId`: `BucketId` (u64)

**Example:**
```
bucketId: 0
```

**Conditions:**
- `now > expires_at + SettlementTimeout`

**Events:** `AgreementEnded` (with full payment to provider)
**Errors:** `AgreementNotFound`, `AgreementNotExpired`

---

## Checkpoint & Challenge

### `checkpoint`

**Client-initiated checkpoint.** Submit an MMR root signed by primary providers.

**Parameters:**
- `bucketId`: `BucketId` (u64)
- `mmrRoot`: `H256` - Merkle Mountain Range root
- `startSeq`: `u64` - starting sequence number
- `leafCount`: `u64` - number of leaves in MMR
- `signatures`: `BoundedVec<(AccountId, MultiSignature), T::MaxPrimaryProviders>` - provider signatures

**Example:**
```
bucketId: 0
mmrRoot: 0x1234567890abcdef...
startSeq: 0
leafCount: 10
signatures: [
  (5GrwvaEF..., 0xsignature1...),
  (5FHneW46..., 0xsignature2...)
]
```

**Requirements:**
- Caller is bucket writer or admin
- At least `minProviders` valid signatures, each signer being a current primary
- If the bucket is frozen, `startSeq ≥ frozen_start_seq`

**Events:** `BucketCheckpointed`
**Errors:** `BucketNotFound`, `NotBucketWriter`, `SnapshotViolatesFrozen`, `ProviderNotInSnapshot`, `InvalidSignature`, `InsufficientSignatures`

---

### `extendCheckpoint`

Add late signatures to the current snapshot (e.g. for primaries that finished signing after the original `checkpoint` call).

**Parameters:**
- `bucketId`: `BucketId` (u64)
- `additionalSignatures`: `BoundedVec<(AccountId, MultiSignature), T::MaxPrimaryProviders>`

**Example:**
```
bucketId: 0
additionalSignatures: [
  (5FLSigC9..., 0xsignature3...)
]
```

**Events:** `BucketCheckpointed` (with the new signer set)
**Errors:** `BucketNotFound`, `NotBucketWriter`, `NoSnapshot`, `ProviderNotInSnapshot`, `InvalidSignature`

---

> **Who may challenge, and what it costs** (applies to `challengeCheckpoint`,
> `challengeOffchain`, and `challengeReplica`). Any signed account may
> challenge, with one restriction: on a `Private` bucket, challenging a
> **primary** provider requires being a bucket member or the owner of a primary
> agreement on the bucket (`NotAuthorizedForPrivateBucket`; replica-agreement
> owners deliberately excluded). The gate keys on the challenged provider's
> role in its current agreement, whichever extrinsic is used; challenging a
> replica is open to everyone.
> The challenger locks a deposit covering the provider's on-chain response
> (over-estimated; excess refunded on resolution). On a valid response the
> provider's **stake is never touched**—only its response tx fee is at issue:
> - **Authorized** (bucket member—Admin/Writer/Reader—or owner of any storage
>   agreement on the bucket): the provider bears a fraction per the
>   `respondToChallenge` cost-split table; the challenger pays the rest, never
>   less than 50%.
> - **General public** (everyone else): pays the fee in full; the provider
>   loses no money.
>
> The tier is fixed at challenge creation (stored in the challenge). A
> failed/absent response slashes the provider's full stake regardless of tier.
> Rationale (leverage-not-recovery, anti-DoS, visibility gating): design doc,
> "The Challenge Game".

### `challengeCheckpoint`

Challenge a primary provider against the **current snapshot** on chain. No signature needed: the snapshot itself is proof of commitment.

**Parameters:**
- `bucketId`: `BucketId` (u64)
- `provider`: `AccountId`
- `leafIndex`: `u64` - which leaf to challenge
- `chunkIndex`: `u64` - which chunk within the leaf

**Example:**
```
bucketId: 0
provider: 5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY
leafIndex: 7
chunkIndex: 3
```

**Deposit required:** see the challenge cost note above.

**Events:** `ChallengeCreated`
**Errors:** `BucketNotFound`, `NoSnapshot`, `ProviderNotInSnapshot`, `NotAuthorizedForPrivateBucket` (private bucket; caller neither member nor primary-agreement owner)

---

### `challengeOffchain`

Challenge a provider using an **off-chain commitment signature**. Works even when the on-chain snapshot has moved on — preferred for hot buckets.

**Parameters:**
- `bucketId`: `BucketId` (u64)
- `provider`: `AccountId`
- `mmrRoot`: `H256`
- `startSeq`: `u64`
- `leafIndex`: `u64`
- `chunkIndex`: `u64`
- `providerSignature`: `MultiSignature` - provider's signature over the commitment payload

**Example:**
```
bucketId: 0
provider: 5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY
mmrRoot: 0x1234567890abcdef...
startSeq: 0
leafIndex: 7
chunkIndex: 3
providerSignature: 0xsig...
```

**Events:** `ChallengeCreated`
**Errors:** `BucketNotFound`, `AgreementNotFound`, `InvalidSignature`, `NotAuthorizedForPrivateBucket` (private bucket, primary target; caller neither member nor primary-agreement owner)

---

### `challengeReplica`

Challenge a **replica** provider against the MMR root recorded by their last `confirmReplicaSync`. No signature needed.

**Parameters:**
- `bucketId`: `BucketId` (u64)
- `provider`: `AccountId`
- `leafIndex`: `u64`
- `chunkIndex`: `u64`

**Example:**
```
bucketId: 0
provider: 5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty
leafIndex: 7
chunkIndex: 3
```

**Events:** `ChallengeCreated`
**Errors:** `AgreementNotFound`, `NotReplica`, `InvalidSyncRoot`

---

### `respondToChallenge`

Provider defends an active challenge. Three response variants:
- `Proof { chunk_data, mmr_proof, chunk_proof }` - present the chunk and Merkle proof
- `Deleted { new_mmr_root, new_start_seq, admin, admin_signature }` - show the bucket admin authorized deletion
- `Superseded` - show the challenged state was overtaken by a newer canonical state

**Parameters:**
- `challengeId`: `ChallengeId<BlockNumber>`
- `response`: `ChallengeResponse<T>`

**Example:**
```
challengeId: { bucket_id: 0, provider: 5Grwva..., created_at: 12345 }
response: Proof {
  chunk_data: 0x...,
  mmr_proof: [0xhash1..., 0xhash2...],
  chunk_proof: [0xhash3...]
}
```

**Cost split by response speed** (challenger funds : provider bears) — applies
only to **authorized** challengers; for the general public it is always 100 / 0.
Numbers are illustrative.

| Response time | Split (challenger : provider) |
|---|---|
| Block 1 | 90 / 10 |
| Blocks 2-5 | 80 / 20 |
| Blocks 6-24 | 70 / 30 |
| Blocks 25-95 | 60 / 40 |
| Blocks 96+ | 50 / 50 |

**Events:** `ChallengeDefended`
**Errors:** `ChallengeNotFound`, `NotChallengeProvider`, `ChallengeExpired`, `InvalidChallengeProof`, `InvalidDeletionProof`, `LeafBeyondCanonical`

---

## Replica Sync

### `confirmReplicaSync`

Replica provider confirms they have synced to one of the bucket's known MMR roots (current snapshot plus 6 historical entries). Deducts `replicaSyncPrice` from the agreement's `syncBalance` and pays the provider.

**Parameters:**
- `bucketId`: `BucketId` (u64)
- `roots`: `[Option<H256>; 7]` - match candidates, indexed `[current, hist_0, hist_1, hist_2, hist_3, hist_4, hist_5]`
- `_signature`: `MultiSignature` - placeholder, currently unused

**Example:**
```
bucketId: 0
roots: [
  Some(0xcurrentRoot...),
  Some(0xhistRoot0...),
  None,
  None,
  None,
  None,
  None
]
_signature: 0x...
```

**Validation:** matched root differs from the previously synced root; at least `minSyncInterval` blocks since last sync; `syncBalance ≥ replicaSyncPrice`.

**Events:** `ReplicaSynced { position_matched, sync_payment }`
**Errors:** `AgreementNotFound`, `NotReplica`, `InvalidSyncRoot`, `SyncTooFrequent`, `InsufficientSyncBalance`

---

### `topUpReplicaSyncBalance`

Reserve additional funds for future sync payments on a replica agreement. Permissionless.

**Parameters:**
- `bucketId`: `BucketId` (u64)
- `provider`: `AccountId`
- `amount`: `Balance` - amount to add to `syncBalance`

**Example:**
```
bucketId: 0
provider: 5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty
amount: 50000000
```

**Events:** `ReplicaSyncBalanceToppedUp`
**Errors:** `AgreementNotFound`, `NotReplica`

---

## Types

### `Role`

```rust
enum Role { Admin, Writer, Reader }
```
- **Admin** - manage members, request primary agreements, configure the bucket, terminate primaries
- **Writer** - submit checkpoints
- **Reader** - read-only

### `Visibility`

```rust
enum Visibility { Public, Private }
```
- **Public** - primaries serve reads to anyone; anyone may challenge primaries
- **Private** - primaries serve reads only to members (cooperative, not chain-enforced); on-chain, primary challenges are restricted to members and primary-agreement owners

Every bucket-creating extrinsic (`establish_storage_agreement`, `create_drive`, `create_s3_bucket`) takes a `visibility` parameter; wrappers that omit the choice default to `Private` (fail-safe).

### `EndAction`

```rust
enum EndAction { Pay, Burn }
```

### `ChallengeResponse<T>`

```rust
enum ChallengeResponse<T> {
    Proof      { chunk_data, mmr_proof, chunk_proof },
    Deleted    { new_mmr_root, new_start_seq, admin, admin_signature },
    Superseded,
}
```

### `RemovalReason`

```rust
enum RemovalReason { Terminated, Expired, Slashed }
```

---

## Well-Known Account IDs

For testing on local development network:

| Account | AccountId | Public Key (Sr25519) | Port |
|---------|-----------|---------------------|------|
| Alice | `5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY` | `0xd43593c715fdd31c61141abd04a99fd6822c8558854ccde39a5684e7a56da27d` | 3333 |
| Bob | `5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty` | `0x8eaf04151687736326c9fea17e25fc5287613693c912909cb226aa4794f26a48` | 3001 |
| Charlie | `5FLSigC9HGRKVhB9FiEo4Y3koPsNmBmLJbpXg2mp1hXcS59Y` | `0x90b5ab205c6974c9ea841be688864633dc9ca8a357843eeacf2314649965fe22` | 3002 |

---

## Common Workflows

### 1. Register & configure provider

```
1. registerProvider(multiaddr, publicKey, stake)
2. updateProviderSettings(settings)  ← required to accept agreements!
```

### 2. Create bucket & request storage

```
1. createBucket(minProviders: 2)
2. requestPrimaryAgreement(bucketId, provider, maxBytes, duration, maxPayment)
3. [provider] acceptAgreement(bucketId)
```

Or in a single call:

```
createBucketWithStorage(maxBytes, duration, maxPricePerByte)
```

### 3. Add replica provider

```
1. [provider with replicaSyncPrice=Some(p)] updateProviderSettings(...)
2. requestAgreement(bucketId, replicaProvider, ..., replicaParams { syncBalance, minSyncInterval })
3. [replica] acceptAgreement(bucketId)
4. [replica syncs from primary off-chain]
5. [replica] confirmReplicaSync(bucketId, roots, _signature)
```

### 4. Client-initiated checkpoint

```
1. [off-chain] collect signatures from primaries
2. checkpoint(bucketId, mmrRoot, startSeq, leafCount, signatures)
3. [optional] extendCheckpoint(...) with late signers
```

### 5. Challenge a provider

```
// against current snapshot:
challengeCheckpoint(bucketId, provider, leafIndex, chunkIndex)

// against an off-chain commitment (hot buckets):
challengeOffchain(bucketId, provider, mmrRoot, startSeq, leafIndex, chunkIndex, providerSignature)

// against a replica:
challengeReplica(bucketId, provider, leafIndex, chunkIndex)

[provider] respondToChallenge(challengeId, response)
```

### 6. Two-step provider exit

```
1. deregisterProvider()        // step 1: announce (committed_bytes must be 0)
2. // wait DeregisterAnnouncementPeriod blocks
3. completeDeregister()        // step 2: unreserve stake, drop record
   // or change your mind:
   cancelDeregister()          // restores acceptingPrimary/Extensions
```

### 7. Clean up a slashed provider

```
[anyone] removeSlashed(bucketId, slashedProvider)   // permissionless
```

---

## Error Reference

Common errors you might encounter:

| Error | Cause | Solution |
|-------|-------|----------|
| `ProviderAlreadyRegistered` | Account already registered | Use a different account |
| `ProviderNotFound` | Account is not a registered provider | Register first |
| `InsufficientStake` | `stake < MinProviderStake` (1000 tokens) | Use stake: `1000000000000000` |
| `InsufficientStakeForBytes` | New commitment would exceed `stake / MinStakePerByte` | Increase stake or reduce bytes |
| `ProviderHasActiveAgreements` | `committed_bytes > 0` at deregister | Wait for agreements to end |
| `ProviderNotAcceptingPrimary` | Provider has `acceptingPrimary=false` | Use a different provider |
| `ProviderNotAcceptingReplicas` | Provider has `replicaSyncPrice=None` | Use a provider that accepts replicas |
| `ProviderNotAcceptingExtensions` | Provider has `acceptingExtensions=false` | Use a different provider |
| `ProviderNotSlashed` | `removeSlashed` called on a non-slashed provider | Only valid when `stake == 0` |
| `CapacityBelowCommitted` | `maxCapacity < committed_bytes` | Wait for agreements to expire or raise capacity |
| `CapacityExceeded` | Agreement would push committed bytes over `maxCapacity` | Find a provider with more headroom |
| `InsufficientStakeForCapacity` | `stake < maxCapacity × MinStakePerByte` | Increase stake or reduce capacity |
| `MinDurationExceedsMaxDuration` | Settings have `minDuration > maxDuration` | Fix the values |
| `DeregisterAnnounced` | Operation blocked during announcement window | Call `cancelDeregister` to undo |
| `DeregisterNotAnnounced` | `completeDeregister` / `cancelDeregister` called before announcing | Call `deregisterProvider` first |
| `DeregisterPeriodNotElapsed` | Too early to complete | Wait for `deregister_at` |
| `BucketNotFound` | Invalid bucket ID | Check bucket exists |
| `BucketFrozen` / `BucketNotFrozen` | Operation requires opposite state | — |
| `NotBucketAdmin` / `NotBucketMember` / `NotBucketWriter` | Caller lacks role | Have an admin assign the role |
| `NotAuthorizedForPrivateBucket` | Private bucket: primary challenged by caller who is neither member nor primary-agreement owner | Become a member, or challenge a replica |
| `MemberNotFound` | Member not in bucket | Add first via `setMember` |
| `CannotDemoteAdmin` | Tried to demote/remove **another** admin | Only an admin can demote themselves |
| `LastAdminCannotBeRemoved` | Demote/remove would leave bucket with zero admins | Promote another admin first |
| `MaxMembersReached` / `MaxPrimaryProvidersReached` / `TooManyBucketsForMember` | Bound hit | — |
| `MinProvidersNotMet` | Snapshot lacks `minProviders` signers | Collect more signatures |
| `InvalidMinProviders` | `minProviders > primary_count` | Lower threshold or add primaries |
| `AgreementNotFound` / `AgreementRequestNotFound` | No matching (bucket, provider) | Create the agreement/request first |
| `AgreementAlreadyExists` / `AgreementRequestAlreadyExists` | One already exists | — |
| `AgreementExpired` / `AgreementNotExpired` | Wrong lifecycle state | — |
| `AgreementExtensionsBlocked` | `setExtensionsBlocked(true)` is active for this agreement | Use a different provider |
| `NotAgreementOwner` | Caller is not the requester | — |
| `DurationTooShort` / `DurationTooLong` | Outside provider's `minDuration` / `maxDuration` | Adjust duration |
| `PaymentExceedsMax` | Calculated payment > `maxPayment` | Calculate `price × bytes × duration`, add 10-20% buffer |
| `RequestExpired` | Request older than `RequestTimeout` | Resubmit |
| `CannotTerminateReplica` | `endAgreement` early-termination tried on a replica | Replicas can't be terminated early |
| `SettlementWindowPassed` | Owner did not call `endAgreement` in time | Provider must use `claimExpiredAgreement` |
| `NotReplica` | Operation requires a replica agreement | — |
| `SyncTooFrequent` | Less than `minSyncInterval` since last sync | Wait |
| `InvalidSyncRoot` | Provided root doesn't match snapshot or last sync | Refresh known roots |
| `InsufficientSyncBalance` | `syncBalance < replicaSyncPrice` | Call `topUpReplicaSyncBalance` |
| `ChallengeNotFound` / `ChallengeAlreadyExists` | — | — |
| `InvalidChallengeProof` / `InvalidDeletionProof` | Proof did not verify | Build proof correctly |
| `ChallengeExpired` | Past challenge deadline | Too late to respond |
| `NotChallengeProvider` | Caller is not the challenged provider | — |
| `ProviderNotInSnapshot` | Provider didn't sign current snapshot | Add their signature via `extendCheckpoint` |
| `LeafBeyondCanonical` | Challenged leaf is beyond canonical state | — |
| `InvalidSignature` | Signature didn't verify against registered pubkey | Check key + payload |
| `NoSnapshot` | Bucket has no checkpoint yet | Call `checkpoint` first |
| `SnapshotViolatesFrozen` | `startSeq < frozen_start_seq` | Use `startSeq ≥ frozen_start_seq` |
| `InsufficientSignatures` | Fewer signatures than `minProviders` | Collect more |
| `NoMatchingProvider` | `createBucketWithStorage` couldn't find a fit | Relax constraints |
| `InvalidMultiaddr` / `InvalidPublicKey` | Malformed input | Fix the value |
| `ArithmeticOverflow` | Internal overflow | Report; should not occur |

---

## Configuration Parameters

Runtime configuration (see [runtimes/web3-storage-local/src/storage.rs](../../runtimes/web3-storage-local/src/storage.rs)).
All durations are in **anchor (relay-chain) blocks** — 6 s each, `RC_HOURS` —
not parachain blocks:

```rust
// Token configuration
UNIT = 1_000_000_000_000       // 1 token (12 decimals)

// Provider
MinProviderStake             = 1_000 * UNIT       // 1000 tokens = 1,000,000,000,000,000
MinStakePerByte              = 1_000              // per byte of declared capacity (1 token per GB)
DeregisterAnnouncementPeriod = 54 * RC_HOURS      // > ChallengeTimeout: 48h window + 6h grace

// Bucket / members
MaxMembers, MaxPrimaryProviders, MaxBucketsPerMember, MaxMultiaddrLength

// Agreement lifecycle
RequestTimeout       = 6 * RC_HOURS    // pending request expiry
SettlementTimeout    = 24 * RC_HOURS   // owner's window after expiry
ChallengeTimeout     = 48 * RC_HOURS   // provider's window to respond
MaxChunkSize         = 256 * 1024      // challenge response chunk cap (256 KiB)
```

**Note:** The runtime uses **12 decimal places** (like Polkadot), so:
- Entering `1000000000000000` in Polkadot.js = 1000 tokens
- Minimum stake to register = 1000 tokens

### Capacity & Stake Calculations

Providers must stake enough to back their declared capacity:

```
required_stake = max_capacity × MinStakePerByte

Example:
  max_capacity = 1 TB = 1,099,511,627,776 bytes
  MinStakePerByte = 1,000
  required_stake = 1,099,511,627,776 × 1,000 = ~1.1 × 10^15 units (~1100 tokens)
```

**Capacity rules:**
- `maxCapacity = 0` means unlimited (no stake requirement for capacity)
- Provider's `committed_bytes` cannot exceed `maxCapacity`
- When accepting agreements, provider's capacity is checked
