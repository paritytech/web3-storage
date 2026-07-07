# S3-Compatible Storage Interface

This module provides an S3-compatible storage interface (Layer 1) on top of the existing Layer 0 blob storage. It offers familiar S3 API semantics while leveraging web3-storage's decentralized, trustless storage guarantees.

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    S3 Client SDK                            │
│    (put_object, get_object, list_objects, etc.)             │
│    Coordinates chain operations + provider blob storage     │
└─────────────────────────────────────────────────────────────┘
        │                                      │
        ▼                                      ▼
┌───────────────────────┐    ┌─────────────────────────────────┐
│   pallet-s3-registry  │    │   Provider Node (Layer 0)       │
│   (On-chain metadata) │    │   (Unchanged - blob storage)    │
│   - S3 bucket info    │    │   - PUT /node                   │
│   - Object key→CID    │    │   - GET /node                   │
│   - Name → ID mapping │    │   - POST /commit                │
└───────────────────────┘    └─────────────────────────────────┘
        │                                      │
        └──────────────┬───────────────────────┘
                       ▼
┌─────────────────────────────────────────────────────────────┐
│                    S3 Primitives                            │
│    (ObjectKey, ObjectMetadata, validation helpers)          │
└─────────────────────────────────────────────────────────────┘
```

## Components

| Component | Path | Description |
|-----------|------|-------------|
| **s3-primitives** | `primitives/` | Core types and validation functions (no_std compatible) |
| **pallet-s3-registry** | `pallet-s3-registry/` | On-chain S3 bucket and object metadata storage |
| **s3-client** | `client/` | High-level SDK for S3 operations |

## Quick Start

```rust
use s3_client::{PutObjectOptions, S3Client, Signer};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create client
    let client = S3Client::new(
        "ws://127.0.0.1:2222",           // Chain URL
        "http://localhost:3333",          // Provider URL
        Signer::from_seed("//Alice")?,    // Signer
    ).await?;

    // Create bucket
    let bucket = client.create_bucket("my-bucket").await?;
    println!("Created bucket: {:?}", bucket);

    // Upload object
    let response = client.put_object(
        "my-bucket",
        "hello.txt",
        b"Hello, Web3 Storage!",
        PutObjectOptions::default(),
    ).await?;
    println!("Uploaded with CID: {:?}", response.cid);

    // Download object
    let data = client.get_object("my-bucket", "hello.txt").await?;
    println!("Downloaded: {}", String::from_utf8_lossy(&data.data));

    // List objects
    let objects = client.list_objects_v2("my-bucket", Default::default()).await?;
    println!("Objects: {:?}", objects);

    Ok(())
}
```

## Detailed Flows

### 1. Bucket Creation Flow

The client only interacts with the S3 pallet. Layer 0 bucket creation is handled internally by the pallet.

```
Client                     S3 Pallet                  Storage Provider Pallet
   │                          │                              │
   │ create_bucket("my-bkt")  │                              │
   │ ─────────────────────────>                              │
   │                          │                              │
   │                          │ create_bucket_internal(who, min_providers)
   │                          │ ─────────────────────────────>
   │                          │                              │
   │                          │        layer0_bucket_id      │
   │                          │ <─────────────────────────────
   │                          │                              │
   │                          │ (stores S3 bucket metadata   │
   │                          │  linking to layer0_bucket_id)│
   │                          │                              │
   │     BucketInfo           │                              │
   │ <─────────────────────────                              │
```

**Key points:**
- Client calls `S3Registry::create_s3_bucket(name, min_providers)`
- S3 pallet validates the bucket name (S3 naming rules: 3-63 chars, lowercase alphanumeric + hyphens)
- S3 pallet internally creates Layer 0 bucket via `pallet_storage_provider::create_bucket_internal()`
- S3 bucket metadata is stored with reference to `layer0_bucket_id`
- Client receives `BucketInfo` containing both S3 and Layer 0 bucket IDs

### 2. Object Upload Flow (put_object)

```
Client                S3 Client SDK         Provider Node         S3 Pallet (Chain)
   │                       │                      │                      │
   │ put_object(bucket,    │                      │                      │
   │   key, data)          │                      │                      │
   │ ──────────────────────>                      │                      │
   │                       │                      │                      │
   │                       │ POST /node (data)    │                      │
   │                       │ ─────────────────────>                      │
   │                       │                      │                      │
   │                       │      CID (hash)      │                      │
   │                       │ <─────────────────────                      │
   │                       │                      │                      │
   │                       │ put_object_metadata(bucket_id, key, CID, size, content_type)
   │                       │ ─────────────────────────────────────────────>
   │                       │                      │                      │
   │   PutObjectResponse   │                      │                      │
   │ <──────────────────────                      │                      │
```

**Key points:**
- Data goes to provider node via HTTP (off-chain, fast)
- Only metadata (key→CID mapping) goes on-chain
- CID is content-addressed hash (blake2-256) - immutable reference to data
- ETag is derived from CID for S3 compatibility

### 3. Object Download Flow (get_object)

```
Client                S3 Client SDK         S3 Pallet (Chain)      Provider Node
   │                       │                      │                      │
   │ get_object(bucket,    │                      │                      │
   │   key)                │                      │                      │
   │ ──────────────────────>                      │                      │
   │                       │                      │                      │
   │                       │ get_object_metadata(bucket_id, key)         │
   │                       │ ─────────────────────>                      │
   │                       │                      │                      │
   │                       │    ObjectMetadata    │                      │
   │                       │    (CID, size, etc)  │                      │
   │                       │ <─────────────────────                      │
   │                       │                      │                      │
   │                       │              GET /node?cid=...              │
   │                       │ ─────────────────────────────────────────────>
   │                       │                      │                      │
   │                       │                           data              │
   │                       │ <─────────────────────────────────────────────
   │                       │                      │                      │
   │  GetObjectResponse    │                      │                      │
   │  (data, metadata)     │                      │                      │
   │ <──────────────────────                      │                      │
```

**Key points:**
- Chain provides the CID (content hash)
- Client fetches actual data from provider using that CID
- Data integrity verified via CID (content-addressed)

### 4. Checkpoints

Checkpoints are how providers commit to the data they're storing. They create an on-chain proof of stored data.

```
Provider Node                              Chain (Storage Provider Pallet)
     │                                              │
     │ (builds MMR over all stored chunks)          │
     │                                              │
     │ submit_checkpoint(bucket_id, mmr_root, sig)  │
     │ ─────────────────────────────────────────────>
     │                                              │
     │                                   (stores checkpoint)
     │                                   (provider now liable for data)
```

**How it works:**
1. Provider builds a Merkle Mountain Range (MMR) over all stored data chunks
2. Provider signs the MMR root and submits checkpoint to chain
3. Once checkpointed, provider is economically committed - they can be challenged/slashed if they lose data
4. Checkpoints happen at Layer 0 level (storage-provider-pallet), not S3 level
5. S3 objects reference Layer 0 data via CID - when Layer 0 data is checkpointed, S3 objects are implicitly covered

**Checkpoint verification:**
- MMR allows efficient proofs for any individual chunk
- Client can request merkle proofs from provider to verify specific data

### 5. Challenges and Slashing

Challenges are the enforcement mechanism - how clients prove a provider lost data.

```
Client                              Chain                           Provider
   │                                  │                                │
   │ (requests data, provider fails)  │                                │
   │                                  │                                │
   │ create_challenge(bucket_id,      │                                │
   │   chunk_id, merkle_proof)        │                                │
   │ ─────────────────────────────────>                                │
   │                                  │                                │
   │                         (challenge created)                       │
   │                         (provider has N blocks to respond)        │
   │                                  │                                │
   │                                  │    "prove you have this data"  │
   │                                  │ ───────────────────────────────>
   │                                  │                                │
   │                                  │                                │
   │         If provider responds with valid proof:                    │
   │         ─────────────────────────────────────────                 │
   │                                  │    proof_of_storage(data)      │
   │                                  │ <───────────────────────────────
   │                         (challenge dismissed)                     │
   │                                  │                                │
   │         If provider fails to respond in time:                     │
   │         ─────────────────────────────────────────                 │
   │                         (provider slashed)                        │
   │                         (stake forfeited)                         │
   │                         (client compensated)                      │
```

**Challenge flow:**
1. Client tries to download data, provider fails to respond or returns wrong data
2. Client submits challenge on-chain with:
   - Bucket/chunk identifier
   - Merkle proof from last checkpoint showing provider committed to having this data
3. Provider has a challenge period (e.g., 100 blocks) to respond with valid data
4. If provider fails: stake is slashed, client receives compensation
5. If provider proves they have data: challenge dismissed

**Why this works:**
- Providers stake tokens when registering
- Checkpoints create on-chain commitments
- Economic incentive: losing stake > cost of storing data
- Chain is "credible threat" - rarely touched, but enforces honesty

## S3-Layer 0 Relationship

```
S3 Layer (pallet-s3-registry)          Layer 0 (storage-provider-pallet)
┌─────────────────────────────┐        ┌──────────────────────────────────┐
│ S3 Bucket                   │        │ Layer 0 Bucket                   │
│  - name: "my-bucket"        │───────>│  - bucket_id: 42                 │
│  - s3_bucket_id: 0          │        │  - owner: Alice                  │
│  - layer0_bucket_id: 42     │        │  - min_providers: 1              │
└─────────────────────────────┘        │  - checkpoints, challenges, etc. │
           │                           └──────────────────────────────────┘
           │                                          │
           ▼                                          ▼
┌─────────────────────────────┐        ┌──────────────────────────────────┐
│ S3 Object                   │        │ Provider Storage                 │
│  - key: "folder/file.txt"   │───────>│  - CID: 0x1234...                │
│  - cid: 0x1234...           │        │  - actual blob data              │
│  - size: 1024               │        │  - MMR inclusion                 │
│  - content_type: text/plain │        │  - checkpoint coverage           │
└─────────────────────────────┘        └──────────────────────────────────┘
```

**Key relationships:**
- S3 provides naming/organization (human-friendly keys)
- Layer 0 provides storage guarantees (checkpoints, challenges, slashing)
- CID links the two - S3 object references Layer 0 data by content hash
- Checkpoints and challenges happen at Layer 0, but protect S3 objects indirectly through CID references

## API Reference

### S3Client Methods

#### Bucket Operations

| Method | Description |
|--------|-------------|
| `create_bucket(name)` | Create a new S3 bucket (1 provider minimum) |
| `create_bucket_with_options(name, min_providers)` | Create bucket with custom provider count |
| `delete_bucket(name)` | Delete an empty bucket |
| `head_bucket(name)` | Get bucket information |
| `list_buckets()` | List all buckets owned by the user |

#### Object Operations

| Method | Description |
|--------|-------------|
| `put_object(bucket, key, data, options)` | Upload an object |
| `get_object(bucket, key)` | Download an object |
| `delete_object(bucket, key)` | Delete an object |
| `head_object(bucket, key)` | Get object metadata without downloading |
| `copy_object(src_bucket, src_key, dst_bucket, dst_key)` | Copy an object |
| `list_objects_v2(bucket, params)` | List objects with prefix/delimiter support |

### PutObjectOptions

```rust
pub struct PutObjectOptions {
    pub content_type: Option<String>,        // MIME type
    pub metadata: HashMap<String, String>,   // User-defined metadata
}
```

### ListObjectsParams

```rust
pub struct ListObjectsParams {
    pub prefix: Option<String>,       // Filter by prefix
    pub delimiter: Option<String>,    // Group by delimiter (e.g., "/")
    pub max_keys: Option<u32>,        // Max results per page
    pub continuation_token: Option<String>,  // Pagination token
}
```

## Testing

```bash
# Test all S3 components
just s3-test-all

# Test individual components
cargo test -p s3-primitives
cargo test -p pallet-s3-registry
cargo test -p s3-client

# Run integration example (requires running infrastructure)
just start-chain                                  # Terminal 1
just start-provider                               # Terminal 2
cargo run -p s3-client --example basic_usage      # Terminal 3
# Or the CI version:
just s3-demo-ci                                   # Terminal 3
```

## Future Enhancements

- **Multipart Upload**: For large files (CreateMultipartUpload, UploadPart, CompleteMultipartUpload)
- **Range Requests**: Partial object downloads (GetObject with byte ranges)
- **Versioning**: Leverage CID immutability to store version history
- **ACLs**: Bucket and object access control policies
- **HTTP Gateway**: Optional S3-compatible HTTP server for AWS CLI compatibility

## License

[Apache-2.0](../../LICENSE-APACHE2)
