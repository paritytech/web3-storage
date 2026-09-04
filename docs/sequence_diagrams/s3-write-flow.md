# S3 Write Flow (putObject)

End-to-end flow from the client uploading an object through chunking, Merkle tree construction, and MMR commit on the primary provider, followed by replica sync.

## 1. Client Upload to Primary Provider

```mermaid
sequenceDiagram
    autonumber
    participant Client as Client (S3 UI)
    participant Chain as Parachain
    participant Primary as Primary Provider (HTTP)
    participant Storage as Storage Backend (RocksDB)

    Client->>Client: Sign request: sr25519_sign(<br/>"web3storage:PUT:{bucket_id}:{timestamp}")

    Client->>Chain: Resolve provider URL:<br/>Buckets(bucket_id).primary_providers[0]<br/>-> Providers(account).multiaddr<br/>-> parse to HTTP URL
    Chain-->>Client: Provider URL (cached per bucket)

    Client->>Primary: PUT /s3/{bucket_id}/object?key=photos/cat.jpg<br/>Authorization: Web3Storage {pubkey}:{sig}:{ts}<br/>Body: raw bytes

    Primary->>Primary: Auth: verify sr25519 signature<br/>+ check Writer role via membership cache<br/>(chain lookup with TTL cache)

    Note over Primary,Storage: Step 1: Chunk data (256 KiB each)

    Primary->>Primary: Split body into N chunks<br/>(N = ceil(size / 256KiB))

    Note over Primary,Storage: Step 2: Store chunks (content-addressed)

    loop For each chunk
        Primary->>Primary: chunk_hash = blake2_256(chunk_data)
        Primary->>Storage: store_node(bucket_id, chunk_hash, data, children=None)
        Storage->>Storage: Verify blake2_256(data) == chunk_hash
        Storage->>Storage: Check quota: used_bytes + len <= max_bytes
        Storage->>Storage: Store in RocksDB CF_NODES keyed by hash
    end

    Note over Primary,Storage: Step 3: Build balanced Merkle tree

    Primary->>Primary: Pad chunk hashes to next power of 2 (zeros)
    loop Bottom-up tree construction
        Primary->>Primary: parent_hash = blake2_256(left || right)
        Primary->>Storage: store_node(bucket_id, parent_hash, data, children=[left, right])
    end
    Primary->>Primary: data_root = tree root hash

    Note over Primary,Storage: Step 4: Commit to MMR

    Primary->>Storage: commit(bucket_id, [data_root])
    Storage->>Storage: Create MmrLeaf{data_root, data_size, total_size}
    Storage->>Storage: leaf_hash = blake2_256(SCALE(MmrLeaf))
    Storage->>Storage: Append to MMR, recalculate root (bag peaks)
    Storage-->>Primary: (mmr_root, start_seq, leaf_indices)

    Note over Primary: Step 5: Update S3 index

    Primary->>Primary: s3_index.put(key -> ObjectMeta{<br/>data_root, size, etag, leaf_index, ...})

    Primary-->>Client: 200 OK {etag, data_root, size, leaf_index}
```

## 2. Replica Sync (Background)

After the primary stores data, replicas pull it autonomously.

```mermaid
sequenceDiagram
    autonumber
    participant Replica as Replica Provider
    participant Chain as Parachain
    participant Primary as Primary Provider (HTTP)

    Note over Replica: ReplicaSyncCoordinator polls every ~12s

    Replica->>Chain: Fetch replica agreements for this provider
    Chain-->>Replica: SyncDuty{bucket_id, target_mmr_root,<br/>primary_endpoints, sync_balance, sync_price}

    Replica->>Replica: Check eligibility:<br/>- sync_balance >= sync_price<br/>- sync_interval elapsed<br/>- local_root != target_root

    Replica->>Primary: GET /mmr_peaks?bucket_id=X
    Primary-->>Replica: peaks[] (root hashes of MMR subtrees)

    loop For each peak, recursively fetch subtree
        Replica->>Primary: GET /node?hash={peak_hash}
        Primary-->>Replica: {data, children}
        Replica->>Replica: Verify blake2_256(data) == hash
        Replica->>Replica: Store node locally

        opt Node has children (internal node)
            Note over Replica,Primary: Recurse: fetch each child<br/>(skip if already in local storage)
        end
    end

    Replica->>Replica: Verify local mmr_root == target_root

    Replica->>Chain: confirm_replica_sync(bucket_id, roots[7])
    Chain->>Chain: Match submitted root against<br/>snapshot + 6 historical_roots
    Chain->>Chain: Deduct sync_price from sync_balance
    Chain->>Chain: Transfer sync_price to replica provider
    Chain->>Chain: Update last_sync = (root, current_block)
    Chain-->>Replica: Event: ReplicaSynced{bucket_id, provider, root}
```

## Data Structure: From Object to MMR

```
Object (raw bytes)
  |
  v
Chunks (256 KiB each, content-addressed by blake2_256)
  [chunk_0] [chunk_1] [chunk_2] [chunk_3] ...
       \       /           \       /
        \     /             \     /
  Balanced Merkle Tree (padded to power of 2)
         [internal]       [internal]
              \               /
               \             /
              data_root (H256)
                    |
                    v
              MmrLeaf { data_root, data_size, total_size }
                    |
                    v
              leaf_hash = blake2_256(SCALE(MmrLeaf))
                    |
                    v
              MMR (append-only, peaks bagged into mmr_root)
```

## Provider URL Resolution

The client resolves which provider to talk to by reading on-chain state:

```
bucket.primary_providers[0]     (AccountId from Buckets storage)
  -> provider.multiaddr          (from Providers storage)
  -> parseMultiaddrToUrl()        (/ip4/127.0.0.1/tcp/3333 -> http://127.0.0.1:3333)
```

Result is cached per bucket. Currently, the client always talks to the **first primary provider** — there is no fallback to replicas on the read/write path.

## Authorization

Requests are signed with sr25519:

```
Authorization: Web3Storage <pubkey_hex>:<signature_hex>:<timestamp>
Signed message: "web3storage:<METHOD>:<bucket_id>:<timestamp>"
```

The provider verifies the signature, resolves the account's role from on-chain bucket membership (cached with TTL), and checks:

| Operation | Required Role |
|-----------|--------------|
| PUT (write) | Writer or Admin |
| GET (read) | Reader, Writer, or Admin |
| DELETE | Writer or Admin |
