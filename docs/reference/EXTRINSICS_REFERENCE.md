# Storage Provider Pallet - Extrinsics Reference

Quick reference for all available extrinsics in the storage provider pallet.

## Provider Management

### `registerProvider`

Register a new storage provider.

**Parameters:**
- `multiaddr`: `BoundedVec<u8>` - Provider's network address (e.g., `/ip4/127.0.0.1/tcp/3000`)
- `publicKey`: `BoundedVec<u8>` - Provider's public key (32 bytes for Sr25519/Ed25519, 33 for ECDSA)
- `stake`: `Balance` - Amount to stake (must be ≥ MinProviderStake)

**Example:**
```
multiaddr: /ip4/127.0.0.1/tcp/3000
publicKey: 0xd43593c715fdd31c61141abd04a99fd6822c8558854ccde39a5684e7a56da27d
stake: 1000000000000000  (1000 tokens, minimum required)
```

**Token Decimals:**
- 1 token = 1,000,000,000,000 (12 decimals, like Polkadot)
- Minimum stake = 1000 tokens = 1,000,000,000,000,000

**Default Settings After Registration:**
- `minDuration`: 0
- `maxDuration`: max_value
- `pricePerByte`: 0
- `acceptingPrimary`: false ⚠️
- `replicaSyncPrice`: None
- `acceptingExtensions`: false
- `maxCapacity`: 0 (unlimited)

⚠️ **Important:** After registration, you must call `updateProviderSettings` to accept agreements!

---

### `updateProviderSettings`

Update provider pricing, availability, and capacity settings.

**Parameters:**
- `settings`: `ProviderSettings`
  - `minDuration`: `BlockNumber` - Minimum agreement duration
  - `maxDuration`: `BlockNumber` - Maximum agreement duration
  - `pricePerByte`: `Balance` - Price per byte per block
  - `acceptingPrimary`: `bool` - Accept new primary agreements
  - `replicaSyncPrice`: `Option<Balance>` - Price for replica sync, or None
  - `acceptingExtensions`: `bool` - Accept agreement extensions
  - `maxCapacity`: `u64` - Maximum storage capacity in bytes (0 = unlimited)

**Example:**
```
settings: {
  minDuration: 100,
  maxDuration: 10000,
  pricePerByte: 1000000,
  acceptingPrimary: true,
  replicaSyncPrice: Some(5000000),
  acceptingExtensions: true,
  maxCapacity: 1099511627776  (1 TB)
}
```

**Capacity Validation:**
- `maxCapacity` cannot be set below current `committed_bytes`
- Provider's stake must be sufficient: `stake >= maxCapacity * MinStakePerByte`
- `maxCapacity = 0` means unlimited capacity (backward compatible)

---

### `deregisterProvider`

Deregister provider (only if no active agreements).

**Parameters:** None

---

### `increaseStake`

Increase provider's staked amount.

**Parameters:**
- `additionalStake`: `Balance` - Amount to add to stake

**Example:**
```
additionalStake: 50000000000
```

---

## Bucket Management

### `createBucket`

Create a new storage bucket.

**Parameters:**
- `minProviders`: `u32` - Minimum number of providers required for checkpoints

**Example:**
```
minProviders: 2
```

**Returns:** Emits `BucketCreated` event with assigned `bucketId`

---

### `addBucketMember`

Add a member to a bucket with a specific role.

**Parameters:**
- `bucketId`: `BucketId` (u64)
- `member`: `AccountId` - Account to add
- `role`: `Role` - One of: `Reader`, `Writer`, `Admin`

**Example:**
```
bucketId: 0
member: 5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty
role: Writer
```

---

### `removeBucketMember`

Remove a member from a bucket.

**Parameters:**
- `bucketId`: `BucketId` (u64)
- `member`: `AccountId`

---

### `freezeBucket`

Freeze a bucket at current snapshot (prevents new uploads after frozen sequence).

**Parameters:**
- `bucketId`: `BucketId` (u64)

⚠️ Requires a snapshot with at least `minProviders` signatures.

---

## Agreement Management

### `requestPrimaryAgreement`

Request a storage agreement with a primary provider.

**Parameters:**
- `bucketId`: `BucketId` (u64)
- `provider`: `AccountId`
- `maxBytes`: `u64` - Maximum storage size
- `duration`: `BlockNumber` - Agreement duration in blocks
- `maxPayment`: `Balance` - Maximum payment willing to pay (safety limit)

**Example:**
```
bucketId: 0
provider: 5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY
maxBytes: 1073741824  (1 GB)
duration: 500
maxPayment: 600000000000000000
```

**Payment Calculation:**
```
payment = price_per_byte × max_bytes × duration
payment = 1,000,000 × 1,073,741,824 × 500
payment = 536,870,912,000,000,000
```

**Important:** `maxPayment` must be ≥ calculated payment, or you'll get `PaymentExceedsMax` error.
Recommended: Add 10-20% buffer to `maxPayment` to account for potential price changes.

---

### `requestReplicaAgreement`

Request a replica agreement (provider syncs from primary).

**Parameters:**
- `bucketId`: `BucketId` (u64)
- `provider`: `AccountId` - Replica provider
- `primaryProvider`: `AccountId` - Source provider for sync
- `duration`: `BlockNumber`

**Example:**
```
bucketId: 0
provider: 5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty
primaryProvider: 5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY
duration: 500
```

---

### `acceptAgreement`

Provider accepts a requested agreement.

**Parameters:**
- `bucketId`: `BucketId` (u64)

**Note:** Caller must be the provider specified in the request.

---

### `rejectAgreement`

Provider rejects a requested agreement.

**Parameters:**
- `bucketId`: `BucketId` (u64)

---

### `requestExtension`

Request to extend an existing agreement.

**Parameters:**
- `bucketId`: `BucketId` (u64)
- `provider`: `AccountId`
- `additionalDuration`: `BlockNumber`

---

### `acceptExtension`

Provider accepts an extension request.

**Parameters:**
- `bucketId`: `BucketId` (u64)
- `requester`: `AccountId` - Who requested the extension

---

### `rejectExtension`

Provider rejects an extension request.

**Parameters:**
- `bucketId`: `BucketId` (u64)
- `requester`: `AccountId`

---

### `burnAgreement`

Burn an expired agreement (can be called by anyone).

**Parameters:**
- `bucketId`: `BucketId` (u64)
- `provider`: `AccountId`

**Conditions:**
- Agreement must be expired
- Remaining payment returned to owner
- Stake returned to provider

---

## Checkpoint & Challenge

### `checkpoint`

Create a checkpoint with provider signatures.

**Parameters:**
- `bucketId`: `BucketId` (u64)
- `mmrRoot`: `H256` - Merkle Mountain Range root
- `startSeq`: `u64` - Starting sequence number
- `leafCount`: `u64` - Number of leaves in MMR
- `signatures`: `BoundedVec<(AccountId, MultiSignature)>` - Provider signatures

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
- At least `minProviders` signatures
- Each signer must be a primary provider
- Signatures must verify against commitment payload

---

### `addSnapshotSignatures`

Add additional signatures to existing snapshot.

**Parameters:**
- `bucketId`: `BucketId` (u64)
- `additionalSignatures`: `BoundedVec<(AccountId, MultiSignature)>`

---

### `challenge`

Challenge a provider to prove they have data.

**Parameters:**
- `bucketId`: `BucketId` (u64)
- `provider`: `AccountId`
- `leafIndex`: `u64` - Which leaf to challenge
- `chunkIndex`: `u64` - Which chunk within the leaf

**Deposit Required:** Challenger must deposit collateral

---

### `respondToChallenge`

Provider responds to a challenge with proof.

**Parameters:**
- `bucketId`: `BucketId` (u64)
- `challenger`: `AccountId`
- `chunkData`: `BoundedVec<u8>` - The challenged chunk
- `proof`: `BoundedVec<H256>` - Merkle proof

---

## Replica Sync

### `confirmReplicaSync`

Primary provider confirms replica sync completion.

**Parameters:**
- `bucketId`: `BucketId` (u64)
- `replicaProvider`: `AccountId`

**Payment:** Transfers `replicaSyncPrice` from bucket owner to primary provider

---

## Well-Known Account IDs

For testing on local development network:

| Account | AccountId | Public Key (Sr25519) | Port |
|---------|-----------|---------------------|------|
| Alice | `5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY` | `0xd43593c715fdd31c61141abd04a99fd6822c8558854ccde39a5684e7a56da27d` | 3000 |
| Bob | `5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty` | `0x8eaf04151687736326c9fea17e25fc5287613693c912909cb226aa4794f26a48` | 3001 |
| Charlie | `5FLSigC9HGRKVhB9FiEo4Y3koPsNmBmLJbpXg2mp1hXcS59Y` | `0x90b5ab205c6974c9ea841be688864633dc9ca8a357843eeacf2314649965fe22` | 3002 |

---

## Common Workflows

### 1. Register & Configure Provider

```
1. registerProvider(multiaddr, publicKey, stake)
2. updateProviderSettings(settings)  ← Required to accept agreements!
```

### 2. Create Bucket & Request Storage

```
1. createBucket(minProviders: 2)
2. requestPrimaryAgreement(bucketId, provider, maxBytes, duration)
3. [Provider] acceptAgreement(bucketId)
```

### 3. Add Replica Provider

```
1. requestReplicaAgreement(bucketId, replicaProvider, primaryProvider, duration)
2. [Replica Provider] acceptAgreement(bucketId)
3. [Replica syncs from primary off-chain]
4. [Primary] confirmReplicaSync(bucketId, replicaProvider)
```

### 4. Create Checkpoint

```
1. [Off-chain] Collect signatures from providers
2. checkpoint(bucketId, mmrRoot, startSeq, leafCount, signatures)
```

### 5. Challenge Provider

```
1. challenge(bucketId, provider, leafIndex, chunkIndex)
2. [Provider] respondToChallenge(bucketId, challenger, chunkData, proof)
```

---

## Error Reference

Common errors you might encounter:

| Error | Cause | Solution |
|-------|-------|----------|
| `ProviderAlreadyRegistered` | Account already registered | Use different account |
| `InsufficientStake` | Stake < MinProviderStake (1000 tokens) | Use stake: `1000000000000000` |
| `BucketNotFound` | Invalid bucket ID | Check bucket exists |
| `NotBucketMember` | Caller not in bucket | Add caller to bucket first |
| `NoSnapshot` | Bucket has no checkpoint | Create checkpoint first |
| `MinProvidersNotMet` | Not enough signatures | Add more provider signatures |
| `ProviderNotInSnapshot` | Provider didn't sign | Ensure provider signed checkpoint |
| `AgreementNotFound` | No agreement exists | Create agreement first |
| `AgreementExpired` | Agreement past expiry | Extend or create new agreement |
| `PaymentExceedsMax` | Calculated payment > maxPayment | Calculate: price × bytes × duration, then add 10-20% buffer |
| `DurationTooShort` | Duration < provider's minDuration | Check provider settings, increase duration |
| `DurationTooLong` | Duration > provider's maxDuration | Check provider settings, decrease duration |
| `CapacityBelowCommitted` | Setting maxCapacity below committed_bytes | Wait for agreements to expire or increase capacity |
| `CapacityExceeded` | Agreement would exceed provider's maxCapacity | Find provider with more available capacity |
| `InsufficientStakeForCapacity` | Stake doesn't cover declared capacity | Increase stake or reduce maxCapacity |

---

## Configuration Parameters

Runtime configuration (see `runtime/src/lib.rs`):

```rust
// Token configuration
UNIT = 1_000_000_000_000       // 1 token (12 decimals)

// Storage provider configuration
MinProviderStake = 1_000 * UNIT  // 1000 tokens = 1,000,000,000,000,000
MaxChunkSize = 256 * 1024        // 256 KiB
ChallengeTimeout = 48 * HOURS    // 48 hours to respond
SettlementTimeout = 24 * HOURS   // 24 hours
RequestTimeout = 6 * HOURS       // 6 hours
MinStakePerByte = 1_000_000      // 1 unit per byte (1 token per MB)
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
  MinStakePerByte = 1,000,000
  required_stake = 1,099,511,627,776 × 1,000,000 = ~1.1 × 10^18 units
```

**Capacity Rules:**
- `max_capacity = 0` means unlimited (no stake requirement for capacity)
- Provider's `committed_bytes` cannot exceed `max_capacity`
- When accepting agreements, provider's capacity is checked
