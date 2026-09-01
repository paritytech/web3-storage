# Extrinsic Execution Flows

This document provides detailed sequence diagrams for all major extrinsics in the Scalable Web3 Storage system, explaining the flow of data between clients, providers, and the blockchain.

Every on-chain block quantity in these diagrams (deadlines, expiries,
timeouts, `current_block`) is denominated on the **anchor clock** — the relay
chain in production, read via `current_anchor_block()`. Pseudocode showing
`frame_system::block_number()` or `current_block` is illustrative; the
implementation reads the anchor. See the anchor-clock section in
[scalable-web3-storage-implementation.md](../design/scalable-web3-storage-implementation.md).

## Table of Contents

1. [Overview](#overview)
2. [Why Checkpoints Require Provider Signatures](#why-checkpoints-require-provider-signatures)
3. [Provider Registration](#provider-registration)
4. [Storage Agreements](#storage-agreements)
5. [Data Upload Flow](#data-upload-flow)
6. [Checkpoint (Commitment) Flow](#checkpoint-commitment-flow)
7. [Data Read Flow](#data-read-flow)
8. [Challenge Flow](#challenge-flow)
9. [Layer 1: Drive Operations](#layer-1-drive-operations)

---

## Overview

The system has a clear separation between:

- **On-chain operations**: Executed as blockchain extrinsics (transactions)
- **Off-chain operations**: HTTP requests to provider nodes

```
┌──────────────────────────────────────────────────────────────────────────┐
│                        Trust Boundaries                                  │
├──────────────────────────────────────────────────────────────────────────┤
│                                                                          │
│  ┌──────────────┐         ┌──────────────┐         ┌──────────────┐      │
│  │    Client    │◄───────►│   Provider   │         │  Blockchain  │      │
│  │              │  HTTP   │    Node      │         │   (Pallet)   │      │
│  └──────────────┘         └──────────────┘         └──────────────┘      │
│         │                        │                        ▲              │
│         │                        │                        │              │
│         └────────────────────────┴────────────────────────┘              │
│                           Extrinsics (signed transactions)               │
│                                                                          │
│  Trust Level:                                                            │
│  • Blockchain: Trustless (consensus-verified)                            │
│  • Provider HTTP: Accountable (signature + stake + challenge)            │
│  • Client: Application-specific                                          │
│                                                                          │
└──────────────────────────────────────────────────────────────────────────┘
```

---

## Why Checkpoints Require Provider Signatures

### The Problem

When a client uploads data to a provider, how do we ensure the provider actually stores it? The provider could:

1. Accept the data, discard it, and claim storage payment
2. Store it initially but delete it later
3. Serve data only when convenient

### The Solution: Signed Commitments

Provider signatures on checkpoints create **non-repudiable evidence**:

```
┌──────────────────────────────────────────────────────────────────────────┐
│  CommitmentPayload (what providers sign)                                 │
├──────────────────────────────────────────────────────────────────────────┤
│  {                                                                       │
│    version: 1,                    // Protocol version                    │
│    bucket_id: u64,                // Which bucket                        │
│    commitment: Commitment {                                              │
│      mmr_root: H256,              // Merkle Mountain Range root          │
│      start_seq: u64,              // First leaf index                    │
│      leaf_count: u64,             // Number of leaves                    │
│    },                                                                    │
│  }                                                                       │
├──────────────────────────────────────────────────────────────────────────┤
│  By signing this, the provider attests:                                  │
│  "I have stored all data corresponding to this MMR root"                 │
│                                                                          │
│  The signature becomes EVIDENCE for:                                     │
│  1. On-chain challenges (challenge_checkpoint)                           │
│  2. Off-chain challenges (challenge_offchain)                            │
│  3. Slashing if provider cannot produce data                             │
└──────────────────────────────────────────────────────────────────────────┘
```

### Why Not Just Trust the Client?

The client could submit a checkpoint claiming the provider stored data, but:

- The provider might not have the data
- There's no evidence linking the provider to the commitment
- Challenges would be unfair (provider didn't agree to store)

**Provider signature = Provider's agreement to be held accountable**

### Multi-Provider Checkpoints

For buckets with multiple providers, we need consensus:

```
┌──────────────────────────────────────────────────────────────────────────┐
│  Checkpoint Threshold Requirement                                        │
├──────────────────────────────────────────────────────────────────────────┤
│                                                                          │
│  Example: Bucket with 3 primary providers                                │
│                                                                          │
│  Provider A signs: ✓                                                     │
│  Provider B signs: ✓                                                     │
│  Provider C signs: ✗ (unavailable)                                       │
│                                                                          │
│  Threshold: 51% must sign                                                │
│  Result: 2/3 = 66.7% ✓ Checkpoint accepted                               │
│                                                                          │
│  Bitfield stored on-chain: 0b00000011                                    │
│  (bit 0 = Provider A, bit 1 = Provider B)                                │
│                                                                          │
│  Only signed providers can be challenged for this checkpoint!            │
│                                                                          │
└──────────────────────────────────────────────────────────────────────────┘
```

---

## Provider Registration

### Extrinsic: `register_provider`

```mermaid
sequenceDiagram
    participant P as Provider
    participant C as Chain (Pallet)
    participant B as Balances

    P->>C: register_provider(multiaddr, public_key, capacity, stake)

    Note over C: Validate inputs
    C->>C: ensure!(stake >= MinProviderStake)
    C->>C: ensure!(public_key is valid format)

    C->>B: hold(HoldReason::ProviderStake, provider, stake)
    Note over B: Stake held under its own reason,<br/>separately from any escrow or deposit<br/>on the same account

    C->>C: Create ProviderInfo
    Note right of C: ProviderInfo {<br/>multiaddr, <br/> public_key,<br/> stake,<br/> committed_bytes: 0, <br/> settings: Default, <br/>stats: Empty <br/>}<br/>

    C->>C: Providers::insert(provider, info)

    C-->>P: Event::ProviderRegistered { provider, stake, capacity }
```

### Extrinsic: `update_provider_settings`

```mermaid
sequenceDiagram
    participant P as Provider
    participant C as Chain (Pallet)

    P->>C: update_provider_settings(settings)

    Note over C: settings = {<br/> min_duration: 100,<br/> max_duration: 100000,<br/> price_per_byte: 1000000,<br/> accepting_primary: true,<br/> replica_sync_price: Some(10M),<br/> accepting_extensions: true<br/>}

    C->>C: info = Providers::get(provider)?
    C->>C: info.settings = new_settings
    C->>C: Providers::insert(provider, info)

    C-->>P: Event::ProviderSettingsUpdated { provider }
```

---

## Storage Agreements

There is no on-chain request/accept round-trip and no standalone bucket
creation: the provider signs a quote (`AgreementTerms`) off-chain, and the
future owner redeems it in a single extrinsic. Redeeming primary terms creates
the bucket together with its primary agreement.

### Extrinsic: `establish_storage_agreement`

```mermaid
sequenceDiagram
    participant P as Provider (off-chain)
    participant O as Owner
    participant C as Chain (Pallet)
    participant B as Balances

    P-->>O: signed AgreementTerms { owner, max_bytes, duration,<br/>price_per_byte, valid_until, nonce,<br/>bucket_id: None, replica_params: None }

    O->>C: establish_storage_agreement(provider, terms, sig)

    Note over C: Validate the quote
    C->>C: ensure!(terms.owner == caller)
    C->>C: ensure!(now <= terms.valid_until <= now + RequestTimeout)
    C->>C: verify sig over blake2_256(PRIMARY_TERM_CONTEXT ++ SCALE(terms))
    C->>C: replay window: try_accept(terms.nonce)

    Note over C: Validate the provider
    C->>C: ensure!(accepting_primary, duration in bounds)
    C->>C: ensure!(committed_bytes + max_bytes fits capacity & stake)

    Note over C: Escrow payment on the owner
    C->>C: payment = price_per_byte × max_bytes × duration
    C->>B: hold(HoldReason::AgreementPayment, owner, payment)

    Note over C: Create bucket + agreement
    C->>C: bucket_id = create_bucket_internal(owner, 1, Some(provider))
    C->>C: StorageAgreements::insert(bucket_id, provider, agreement)

    C-->>O: Event::BucketCreated { bucket_id, admin: owner }
    C-->>O: Event::StorageAgreementEstablished { bucket_id, provider, owner, terms, expires_at }
```

### Extrinsic: `establish_replica_agreement`

Same signed-quote shape against an **existing** bucket: `terms.bucket_id`
must name the target bucket and `terms.replica_params` must be
`Some(ReplicaTerms { sync_balance, sync_price, min_sync_interval })`. The hold
placed on the owner is the storage payment **plus** `sync_balance`; each
accepted `confirm_replica_sync` settles `sync_price` out of it, and whatever
sync balance is unspent when the agreement is removed is released back to the
owner.

---

## Data Upload Flow

This is the primary off-chain flow where data is actually stored:

```mermaid
sequenceDiagram
    participant U as User
    participant SC as Storage Client
    participant PN as Provider Node
    participant S as Storage Layer

    Note over U,S: Step 1: Upload Chunks
    U->>SC: upload(bucket_id, data)

    SC->>SC: Split data into 256 KiB chunks
    SC->>SC: Build Merkle tree of chunks
    SC->>SC: data_root = merkle_root(chunks)

    loop For each chunk
        SC->>PN: PUT /node { bucket_id, hash, data }
        PN->>S: store_node(bucket_id, hash, data)
        PN-->>SC: { stored: true }
    end

    Note over U,S: Step 2: Commit to MMR
    SC->>PN: POST /commit { bucket_id, data_roots: [data_root] }

    PN->>S: Add data_root as new MMR leaf
    PN->>S: Update MMR root
    PN->>PN: Sign commitment payload

    Note over PN: CommitmentPayload {<br/> version: 1,<br/> bucket_id,<br/> commitment: { mmr_root, start_seq, leaf_count }<br/>}

    PN-->>SC: { mmr_root, start_seq, leaf_indices, provider_signature }

    SC-->>U: data_root (CID)
```

---

## Checkpoint (Commitment) Flow

### Extrinsic: `checkpoint`

This is how off-chain state becomes on-chain:

```mermaid
sequenceDiagram
    participant U as User
    participant SC as Storage Client
    participant PN as Provider Node(s)
    participant C as Chain (Pallet)

    Note over U,C: Step 1: Collect signatures from providers

    loop For each primary provider
        SC->>PN: GET /commitment?bucket_id=X
        PN->>PN: Sign CommitmentPayload
        PN-->>SC: { mmr_root, start_seq, provider_signature }
    end

    Note over SC: Verify all providers agree on same mmr_root

    Note over U,C: Step 2: Submit checkpoint on-chain

    U->>C: checkpoint(bucket_id, commitment)

    Note over C: commitment = Commitment { mmr_root, start_seq, leaf_count }

    Note over C: signatures = [(provider1, sig1), (provider2, sig2), ...]

    C->>C: bucket = Buckets::get(bucket_id)?

    loop For each (provider, signature)
        Note over C: Verify provider is in bucket
        C->>C: idx = bucket.primary_providers.position(provider)?

        Note over C: Build payload
        C->>C: payload = CommitmentPayload::new(bucket_id, commitment)

        Note over C: Verify signature against provider's public key
        C->>C: provider_info = Providers::get(provider)?
        C->>C: verify_signature(signature, payload.encode(), provider_info.public_key)?

        Note over C: Mark provider as signed (bitfield)
        C->>C: primary_signers[idx / 8] |= 1 << (idx % 8)
    end

    Note over C: Check threshold (51% of providers)
    C->>C: ensure!(signing_count >= bucket.min_providers * 51%)

    Note over C: Create/update snapshot
    Note over C: bucket.snapshot = Some(BucketSnapshot {<br/> mmr_root,<br/> start_seq,<br/> leaf_count,<br/> checkpoint_block: current_block,<br/> primary_signers<br/>})

    C-->>U: Event::BucketCheckpointed { bucket_id, commitment, providers }
```

### Why Signature Verification Matters

```
┌──────────────────────────────────────────────────────────────────────────┐
│  Signature Verification Flow                                             │
├──────────────────────────────────────────────────────────────────────────┤
│                                                                          │
│  1. Provider registers with public_key                                   │
│     Providers::insert(provider_id, { public_key, ... })                  │
│                                                                          │
│  2. Provider signs commitment off-chain                                  │
│     signature = sr25519_sign(private_key, CommitmentPayload.encode())    │
│                                                                          │
│  3. On-chain verification                                                │
│     sr25519_verify(signature, payload, stored_public_key)                │
│                                                                          │
│  This ensures:                                                           │
│  • Only the registered provider could have signed                        │
│  • Provider agreed to store this specific data (mmr_root)                │
│  • Provider can be held accountable (challenged/slashed)                 │
│                                                                          │
└──────────────────────────────────────────────────────────────────────────┘
```

---

## Data Read Flow

```mermaid
sequenceDiagram
    participant U as User
    participant SC as Storage Client
    participant PN as Provider Node
    participant S as Storage Layer

    U->>SC: read(data_root, offset, length)

    SC->>PN: GET /read?data_root=0x...&offset=0&length=1000000

    Note over PN: Calculate which chunks needed
    PN->>PN: start_chunk = offset / 256KB
    PN->>PN: end_chunk = (offset + length) / 256KB

    loop For each chunk index
        PN->>S: get_chunk_at_index(data_root, chunk_idx)
        S-->>PN: (chunk_data, merkle_proof)
    end

    PN-->>SC: { chunks: [{ hash, data, proof }, ...] }

    Note over SC: Verify each chunk
    loop For each chunk
        SC->>SC: actual_hash = blake2_256(chunk_data)
        SC->>SC: ensure!(actual_hash == expected_hash)
        SC->>SC: verify_merkle_proof(hash, proof, data_root)
    end

    SC->>SC: Reassemble data from chunks
    SC->>SC: Trim to requested range [offset, offset+length]

    SC-->>U: data bytes
```

---

## Challenge Flow

### Extrinsic: `challenge_checkpoint`

When a user suspects data loss:

```mermaid
sequenceDiagram
    participant U as Challenger
    participant C as Chain (Pallet)
    participant P as Provider

    U->>C: challenge_checkpoint(bucket_id, provider, leaf_index, chunk_index)

    Note over C: Verify provider signed the snapshot
    C->>C: bucket = Buckets::get(bucket_id)?
    C->>C: snapshot = bucket.snapshot?
    C->>C: provider_idx = bucket.primary_providers.position(provider)?
    C->>C: ensure!(snapshot.has_provider_signed(provider_idx))

    Note over C: Obligation gate: the agreement must still be live
    C->>C: agreement = StorageAgreements::get(bucket_id, provider)?
    C->>C: ensure!(now < agreement.expires_at)

    Note over C: Create challenge
    C->>C: deadline = current_anchor_block + ChallengeTimeout
    Note over C: challenge = Challenge {<br/> challenger,<br/> bucket_id,<br/> provider,<br/> mmr_root: snapshot.mmr_root,<br/> start_seq: snapshot.start_seq,<br/> target: { leaf_index, chunk_index },<br/> deposit<br/>}

    C->>C: Challenges::insert(deadline, next_index, challenge)

    C-->>U: Event::ChallengeCreated { challenge_id, deadline }
    C-->>P: Event::ChallengeCreated { ... }  // Provider monitors events
```

### Extrinsic: `respond_to_challenge`

Provider must prove they have the data:

```mermaid
sequenceDiagram
    participant P as Provider
    participant PN as Provider Node
    participant C as Chain (Pallet)

    Note over P: Provider detects challenge event

    P->>PN: GET /mmr_proof?bucket_id=X&leaf_index=Y
    PN-->>P: { leaf: { data_root, data_size }, peaks, proof }

    P->>PN: GET /chunk_proof?data_root=0x...&chunk_index=Z
    PN-->>P: { chunk_hash, proof }

    P->>PN: GET /node?hash=<chunk_hash>
    PN-->>P: { data: <actual chunk bytes> }

    P->>C: respond_to_challenge(challenge_id, response)

    Note over C: response = ChallengeResponse::Proof {<br/> chunk_data,<br/> chunk_proof, // Merkle proof chunk → data_root<br/> mmr_proof // MMR proof data_root → mmr_root<br/>}

    Note over C: Verify proofs
    C->>C: chunk_hash = blake2_256(chunk_data)
    C->>C: verify_merkle_proof(chunk_hash, chunk_proof, data_root)?
    C->>C: verify_mmr_proof(mmr_proof, mmr_root)?

    Note over C: Challenge defended! Stake untouched.
    C->>C: Remove challenge
    Note over C: Split the deposit by response time: the provider is paid<br/>10–50% of it for the work of responding (slower response →<br/>bigger share), straight out of the ChallengeDeposit hold
    C->>C: Release the remaining deposit to the challenger

    C-->>P: Event::ChallengeDefended { challenge_id }
```

### Automatic Slashing (if no response)

```mermaid
sequenceDiagram
    participant C as Chain (Pallet)
    participant B as Balances

    Note over C: on_initialize(n) slash sweep

    Note over C: Deadlines are anchor (relay-chain) blocks. Drain every deadline<br/>key after LastSweptChallengeBlock and before the current anchor<br/>block (exclusive — challenges at the anchor itself are still<br/>respondable), budget-capped.

    C->>C: expired = Challenges::drain_prefix(deadline_key)

    loop For each expired challenge
        Note over C: Provider failed to respond!

        C->>C: Slash provider's entire stake
        C->>B: slash(HoldReason::ProviderStake, provider, stake)
        C->>B: resolve(Treasury, credit)

        C->>C: Refund challenger deposit (no reward)
        C->>B: release(HoldReason::ChallengeDeposit, challenger, deposit)

        C->>C: Update provider + challenger stats

        C-->>C: Event::ChallengeSlashed { challenge_id, provider,<br/>slashed_amount, challenger_reward: 0, reason: Timeout }
    end

    Note over C: Bucket membership and agreement teardown are NOT part of the<br/>sweep — anyone triggers them afterwards via the permissionless<br/>remove_slashed extrinsic.
```

---

## Layer 1: Drive Operations

### Extrinsic: `create_drive` (Drive Registry Pallet)

```mermaid
sequenceDiagram
    participant U as User
    participant DR as Drive Registry Pallet
    participant SP as Storage Provider Pallet

    Note over U: Obtain provider-signed AgreementTerms off-chain
    U->>DR: create_drive(name, provider, terms, sig)

    Note over DR: Validate inputs
    DR->>DR: ensure!(name fits, drive count < MaxDrivesPerUser)

    Note over DR: Open bucket + primary agreement atomically via Layer 0
    DR->>SP: establish_storage_agreement_internal(owner, provider, terms, sig)
    Note over SP: signature, replay window, capacity/stake/duration<br/>checks; escrows payment; creates bucket + agreement
    SP-->>DR: bucket_id

    Note over DR: Store drive info
    Note over DR: drive = DriveInfo {<br/> owner,<br/> bucket_id,<br/> created_at,<br/> name,<br/> max_capacity: terms.max_bytes,<br/> storage_period: terms.duration,<br/> expires_at<br/>}

    DR->>DR: Drives::insert(drive_id, drive)
    DR->>DR: UserDrives::append(owner, drive_id)
    DR->>DR: BucketToDrive::insert(bucket_id, drive_id)

    DR-->>U: Event::DriveCreated { drive_id, owner, bucket_id }
```

### Extrinsic: `update_root_cid`

```mermaid
sequenceDiagram
    participant U as User
    participant FSC as File System Client
    participant PN as Provider Node
    participant DR as Drive Registry Pallet

    Note over U,DR: After file operations, root CID changes

    U->>FSC: upload_file(drive_id, "/docs/report.pdf", data)

    Note over FSC: Update directory tree
    FSC->>PN: Upload file chunks
    FSC->>PN: Upload file manifest
    FSC->>PN: Upload updated /docs directory
    FSC->>PN: Upload updated / root directory
    FSC->>PN: POST /commit (get signature)
    PN-->>FSC: new_root_cid, provider_signature

    Note over FSC: Based on CommitStrategy
    alt Immediate
        FSC->>DR: update_root_cid(drive_id, new_root_cid)
    else Batched
        FSC->>FSC: Queue update, submit on interval
    else Manual
        FSC->>FSC: Store pending, wait for user
    end

    U->>DR: update_root_cid(drive_id, new_root_cid)

    DR->>DR: drive = Drives::get(drive_id)?
    DR->>DR: ensure!(drive.owner == caller)
    DR->>DR: old_cid = drive.root_cid
    DR->>DR: drive.root_cid = new_root_cid
    DR->>DR: drive.last_committed_at = current_block
    DR->>DR: Drives::insert(drive_id, drive)

    DR-->>U: Event::RootCIDUpdated { drive_id, old_cid, new_root_cid }
```

---

## Summary: Signature Role in the System

```
┌──────────────────────────────────────────────────────────────────────────┐
│  Why Signatures at Each Step                                             │
├──────────────────────────────────────────────────────────────────────────┤
│                                                                          │
│  1. Provider Registration                                                │
│     └─ Provider registers public_key on-chain                            │
│     └─ Establishes identity for signature verification                   │
│                                                                          │
│  2. Off-chain Commit                                                     │
│     └─ Provider signs CommitmentPayload                                  │
│     └─ Client stores signature as proof of provider's agreement          │
│                                                                          │
│  3. On-chain Checkpoint                                                  │
│     └─ Client submits provider signatures                                │
│     └─ Chain verifies each signature against provider's public_key       │
│     └─ Creates non-repudiable record of what provider claimed to store   │
│                                                                          │
│  4. Challenge                                                            │
│     └─ Anyone can challenge providers who signed the checkpoint          │
│     └─ Signature proves provider agreed to be accountable                │
│     └─ Provider must prove data or lose stake                            │
│                                                                          │
│  5. Off-chain Challenge (challenge_offchain)                             │
│     └─ For data not yet checkpointed on-chain                            │
│     └─ Client provides provider's signature from /commit response        │
│     └─ Chain verifies signature, creates challenge                       │
│                                                                          │
│  Result: Signatures create a chain of accountability                     │
│  Provider → "I have this data" (signature)                               │
│  Chain → "Prove it or lose stake" (challenge)                            │
│  Provider → "Here's the proof" OR → Slashed                              │
│                                                                          │
└──────────────────────────────────────────────────────────────────────────┘
```

---

## API Reference Links

- **[Layer 0 Extrinsics Reference](../reference/EXTRINSICS_REFERENCE.md)** - Complete pallet API
- **[Layer 1 API Reference](../filesystems/API_REFERENCE.md)** - File System API
- **[Architecture Overview](../filesystems/ARCHITECTURE.md)** - System architecture
- **[Admin Guide](../filesystems/ADMIN_GUIDE.md)** - System administration

---

*Last updated: February 2026*
