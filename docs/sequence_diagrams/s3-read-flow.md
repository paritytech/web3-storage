# S3 Read Flow (getObject)

End-to-end flow from the client requesting an object through provider URL resolution, authorization, Merkle tree traversal, and data reassembly.

## Read Flow

```mermaid
sequenceDiagram
    autonumber
    participant Client as Client (S3 UI)
    participant Chain as Parachain
    participant Primary as Primary Provider (HTTP)
    participant Storage as Storage Backend (RocksDB)

    Client->>Client: Sign request: sr25519_sign(<br/>"web3storage:GET:{bucket_id}:{timestamp}")

    Client->>Chain: Resolve provider URL (cached):<br/>Buckets.primary_providers[0]<br/>-> Providers.multiaddr -> HTTP URL
    Chain-->>Client: Provider URL

    Client->>Primary: GET /s3/{bucket_id}/object?key=photos/cat.jpg<br/>Authorization: Web3Storage {pubkey}:{sig}:{ts}

    Primary->>Primary: Auth: verify sr25519 signature<br/>+ check Reader role via membership cache

    Primary->>Primary: S3 index lookup: key -> ObjectMeta{<br/>data_root, size, content_type, etag, leaf_index}

    alt Key not found
        Primary-->>Client: 404 ObjectNotFound
    end

    Note over Primary,Storage: DFS traversal from data_root to collect chunks

    Primary->>Storage: collect_chunks(data_root)

    rect rgb(245, 245, 255)
    Note over Storage: Iterative DFS (stack-based)
    Storage->>Storage: Push data_root onto stack

    loop While stack not empty
        Storage->>Storage: Pop hash from stack
        Storage->>Storage: Fetch node from RocksDB (CF_NODES)

        alt Internal node (has children)
            Storage->>Storage: Push children onto stack (reversed for order)
        else Leaf node (no children)
            Storage->>Storage: Append chunk data to result
        end
    end
    end

    Storage-->>Primary: chunks[] (ordered left-to-right)

    Primary->>Primary: Concatenate chunks
    Primary->>Primary: Truncate to original size (remove Merkle padding)

    Primary-->>Client: 200 OK<br/>Content-Type: {content_type}<br/>ETag: {etag}<br/>Body: raw bytes
```

## Rust Client (Layer 0) — Verified Download

The Rust SDK downloads with chunk-level verification, unlike the S3 API which trusts the provider.

```mermaid
sequenceDiagram
    autonumber
    participant RustClient as Rust Client SDK
    participant Primary as Primary Provider (HTTP)

    RustClient->>Primary: GET /read?data_root={hash}&offset=0&length=N

    Primary-->>RustClient: chunks[] with Merkle proofs

    loop For each chunk
        RustClient->>RustClient: Verify blake2_256(chunk_data) == expected_hash
        RustClient->>RustClient: Verify Merkle proof up to data_root
    end

    RustClient->>RustClient: Reassemble, trim to requested range
    RustClient->>RustClient: Decrypt if encryption key set
```

## How Multiple Providers Serve the Same Bucket

```mermaid
sequenceDiagram
    autonumber
    participant Client as Client
    participant Chain as Parachain
    participant P1 as Primary 1
    participant P2 as Primary 2
    participant R1 as Replica 1

    Note over Client,Chain: Provider resolution

    Client->>Chain: Buckets(id).primary_providers
    Chain-->>Client: [P1_account, P2_account]
    Client->>Chain: Providers(P1_account).multiaddr
    Chain-->>Client: /ip4/.../tcp/3333
    Client->>Client: Use P1 (first primary)

    Note over Client,P1: Normal read/write path
    Client->>P1: PUT /s3/{id}/object (write)
    Client->>P1: GET /s3/{id}/object (read)

    Note over P1,P2: Checkpoint coordination (both sign)
    P1->>P2: POST /checkpoint/sign (agree on MMR state)

    Note over P1,R1: Replica sync (background)
    R1->>P1: GET /mmr_peaks + GET /node (pull data)
    R1->>Chain: confirm_replica_sync (get paid)

    Note over Client: Currently no client-side fallback<br/>to P2 or R1 if P1 is down.<br/>Replicas serve as a data availability<br/>backstop, not a read path.
```

## Key Differences: S3 API vs Layer 0 API

| Aspect | S3 API (TypeScript) | Layer 0 API (Rust) |
|--------|--------------------|--------------------|
| Upload | Single PUT request (auto-chunks, auto-commits) | Multi-step: upload chunks, build tree, commit separately |
| Download | Returns raw bytes | Returns chunks with Merkle proofs |
| Verification | Trusts provider | Client verifies each chunk hash |
| Provider resolution | Dynamic from chain (cached) | Static URL list in config |
| Encryption | Done in client state layer before upload | Built into SDK with `ChunkingStrategy` |
