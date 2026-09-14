# S3 Bucket Creation

End-to-end flow from the UI through off-chain negotiation to a single atomic on-chain extrinsic that creates a Layer 0 bucket, a storage agreement, and an S3 registry entry.

## 1. Provider Discovery

```mermaid
sequenceDiagram
    autonumber
    participant UI as S3 UI (NewBucketDialog)
    participant Chain as Parachain

    UI->>Chain: StorageProviderApi.find_matching_providers(<br/>{bytesNeeded, minDuration, maxPricePerByte, primaryOnly}, limit)
    Chain->>Chain: Score providers 0-100 based on:<br/>accepting status, capacity, price, duration range
    Chain-->>UI: MatchingProviders[] sorted by matchScore

    UI->>UI: Display provider list with capacity,<br/>price, reputation stats
    UI->>UI: User selects a provider
```

## 2. Off-Chain Negotiation

```mermaid
sequenceDiagram
    autonumber
    participant UI as S3 UI
    participant Provider as Provider Node (HTTP)

    UI->>UI: parseMultiaddrToHttp(provider.multiaddr)

    UI->>Provider: POST /negotiate {owner, max_bytes,<br/>duration, price_per_byte, bucket_id: null}

    Provider->>Provider: Validate against on-chain settings:<br/>- accepting_primary == true<br/>- price >= provider's listed price<br/>- duration in [min_duration, max_duration]<br/>- capacity not exceeded

    Provider->>Provider: Build AgreementTerms {<br/>  owner, max_bytes, duration,<br/>  price_per_byte (provider's own price),<br/>  valid_until, nonce (monotonic),<br/>  bucket_id: None, replica_params: None<br/>}

    Provider->>Provider: Sign: sr25519_sign(<br/>blake2_256("primary-term-v1:" | SCALE(terms)))

    Provider-->>UI: SignedTerms { terms, signature }
```

## 3. On-Chain Submission (Single Atomic Extrinsic)

```mermaid
sequenceDiagram
    autonumber
    participant UI as S3 UI
    participant Chain as Parachain
    participant SP as StorageProvider Pallet
    participant S3 as S3Registry Pallet

    UI->>Chain: S3Registry.create_s3_bucket(name, provider, terms, sig)

    Note over S3: S3Registry pallet validates name
    S3->>S3: validate_bucket_name (3-63 chars, lowercase + hyphens)
    S3->>S3: Check name uniqueness (BucketNameToId)
    S3->>S3: Check user bucket limit (MaxBucketsPerUser)

    Note over S3,SP: Delegates to Layer 0 pallet
    S3->>SP: establish_storage_agreement_internal(owner, provider, terms, sig)

    SP->>SP: Verify owner == terms.owner
    SP->>SP: Verify bucket_id is None (new primary bucket)
    SP->>SP: Verify terms not expired (valid_until >= current_block)
    SP->>SP: Verify provider signature:<br/>blake2_256("primary-term-v1:" | SCALE(terms))
    SP->>SP: Replay protection: nonce sliding window check
    SP->>SP: Provider active + accepting_primary
    SP->>SP: Duration in [min_duration, max_duration]
    SP->>SP: Capacity: committed_bytes + max_bytes <= max_capacity
    SP->>SP: Stake: provider.stake >= (new_committed * MinStakePerByte)

    SP->>SP: Reserve payment from owner:<br/>price_per_byte * max_bytes * duration

    SP->>SP: Create Layer 0 bucket (owner as Admin, provider as primary)
    SP-->>Chain: Event: BucketCreated{bucket_id, admin}

    SP->>SP: Insert StorageAgreement{max_bytes, payment, expires_at, ...}
    SP->>SP: Update provider: committed_bytes, stats
    SP-->>Chain: Event: StorageAgreementEstablished{<br/>bucket_id, provider, owner, terms, expires_at}

    SP-->>S3: return layer0_bucket_id

    Note over S3: S3Registry creates the S3 layer entry
    S3->>S3: Allocate s3_bucket_id (auto-increment)
    S3->>S3: Store S3BucketInfo{s3_bucket_id, name,<br/>layer0_bucket_id, owner, created_at}
    S3->>S3: Insert BucketNameToId, update UserBuckets
    S3-->>Chain: Event: S3BucketCreated{<br/>s3_bucket_id, name, layer0_bucket_id, owner}

    Chain-->>UI: Transaction finalized + 3 events
```

## 4. UI Completion

```mermaid
sequenceDiagram
    autonumber
    participant UI as S3 UI

    UI->>UI: Extract S3BucketCreated event<br/>(s3_bucket_id, layer0_bucket_id)
    UI->>UI: Cache provider URL keyed by layer0_bucket_id
    UI->>UI: Update creation status to "ready"
    UI->>UI: refreshBuckets() — reload full bucket list
    UI->>UI: selectBucket(new bucket) — auto-select
    UI->>UI: refreshBalance() — update on-chain balance<br/>(payment was reserved)

    Note over UI: Bucket is ready for uploads
```

## Events Emitted (in order)

A successful `create_s3_bucket` extrinsic emits three events:

| # | Pallet | Event | Key Fields |
|---|--------|-------|------------|
| 1 | `StorageProvider` | `BucketCreated` | `bucket_id`, `admin` |
| 2 | `StorageProvider` | `StorageAgreementEstablished` | `bucket_id`, `provider`, `owner`, `terms`, `expires_at` |
| 3 | `S3Registry` | `S3BucketCreated` | `s3_bucket_id`, `name`, `layer0_bucket_id`, `owner` |

## On-Chain Storage Created

| Pallet | Storage Item | Key | Value |
|--------|-------------|-----|-------|
| `StorageProvider` | `Buckets` | `bucket_id` | `Bucket { members, primary_providers, snapshot, ... }` |
| `StorageProvider` | `MemberBuckets` | `account_id` | `BoundedVec<BucketId>` |
| `StorageProvider` | `StorageAgreements` | `(bucket_id, provider)` | `StorageAgreement { max_bytes, payment_locked, expires_at, ... }` |
| `StorageProvider` | `Providers` | `provider` | Updated `committed_bytes` and `stats` |
| `StorageProvider` | `ProviderReplayStates` | `provider` | Nonce window advanced |
| `S3Registry` | `S3Buckets` | `s3_bucket_id` | `S3BucketInfo { name, layer0_bucket_id, owner, ... }` |
| `S3Registry` | `BucketNameToId` | `name` | `s3_bucket_id` |
| `S3Registry` | `UserBuckets` | `account_id` | `BoundedVec<S3BucketId>` |
| `S3Registry` | `NextS3BucketId` | (value) | Incremented by 1 |

## Payment Calculation

```
payment = price_per_byte * max_bytes * duration
```

The payment is reserved (locked) from the owner's account at bucket creation time. The provider's listed `price_per_byte` is used (set during negotiation, not the client's proposed price).

## Replay Protection

The provider allocates a monotonic nonce for each negotiation. On-chain, a sliding window (`ProviderReplayState`) tracks used nonces. This prevents replay attacks where a stale `SignedTerms` could be submitted again.
