# S3 Metadata Index — Design Document

## Problem

S3 bucket creation happens on-chain (extrinsic), but object operations (PUT/GET/DELETE/LIST) must be fast, fee-free HTTP calls through the provider node. The Layer 0 provider node handles content-addressed blob storage (chunks + MMR) but has no concept of object keys, metadata, or listing. Storing per-object metadata on-chain would be prohibitively slow and expensive.

## Architecture

```
Client ──HTTP──> Provider Node ──on-chain──> Parachain
                     │
                     ├── Storage (Layer 0: chunks, Merkle trees, MMR)
                     └── S3IndexManager (Layer 1: key→metadata BTreeMap)
```

- **On-chain**: Bucket creation, agreements, checkpoints, challenges
- **Off-chain (provider node)**: Object PUT/GET/DELETE/LIST, metadata index, periodic Merkle commitment

## Data Structures

### ObjectMeta

Per-object metadata stored in the index:

```rust
pub struct ObjectMeta {
    pub data_root: H256,                    // Merkle tree root of chunked data
    pub size: u64,                          // Original data size
    pub content_type: String,               // MIME type
    pub etag: String,                       // hex(data_root)
    pub last_modified: u64,                 // Unix epoch seconds
    pub user_metadata: Vec<(String, String)>, // x-amz-meta-* key-value pairs
    pub leaf_index: u64,                    // MMR leaf index from commit
}
```

### BucketIndex

Per-bucket sorted key→metadata map using `BTreeMap<String, ObjectMeta>`:

- **put(key, meta)** — insert/update, returns old if overwrite
- **get(key)** — lookup
- **delete(key)** — remove (data stays in MMR)
- **list(prefix, delimiter, start_after, continuation_token, max_keys)** — S3-compatible listing via BTreeMap range scan
- **metadata_merkle_root()** — deterministic Merkle root over sorted entries

### S3IndexManager

Thread-safe wrapper using `DashMap<BucketId, RwLock<BucketIndex>>`:

- Per-bucket locking for concurrent access
- Optional JSON file persistence (atomic writes via temp file + rename)
- Loaded into `ProviderState` and shared across all HTTP handlers

## HTTP API

Object key is passed as a `?key=` query parameter.

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/s3/:bucket_id/object?key=...` | PUT | Upload: chunk → Merkle tree → MMR commit → index update |
| `/s3/:bucket_id/object?key=...` | GET | Reassemble data from chunks, return with Content-Type |
| `/s3/:bucket_id/object?key=...` | HEAD | Return metadata headers only |
| `/s3/:bucket_id/object?key=...` | DELETE | Remove from index (data stays in MMR) |
| `/s3/:bucket_id/objects` | GET | List objects with prefix, delimiter, pagination |
| `/s3/:bucket_id/index_root` | GET | Metadata Merkle root + stats |

### PUT Flow (Server-Side Chunking)

1. Read raw body bytes
2. Split into 256 KiB chunks
3. Hash + store each chunk via `Storage::store_node()`
4. Build balanced Merkle tree via `build_padded_merkle_tree()`
5. Commit data_root to MMR via `Storage::commit()`
6. Extract content_type and x-amz-meta-* headers
7. Create `ObjectMeta`, insert into index, persist to disk
8. Return `{ etag, data_root, size, leaf_index }`

### GET Flow

1. Lookup `ObjectMeta` in index
2. Call `Storage::collect_chunks(data_root)` to reassemble data
3. Return bytes with Content-Type header

### LIST Semantics

Supports S3-compatible listing:
- **prefix** — filter keys by prefix
- **delimiter** — group keys into common prefixes (e.g., `/` for "folder" listing)
- **max_keys** — pagination size (default 1000)
- **continuation_token** — cursor for next page
- **start_after** — skip keys before this value

## Commitment Model

The metadata index produces a deterministic Merkle root:

```
metadata_merkle_root = MerkleTree(sorted[(key, data_root, size) for each object])
```

This root can be committed as a special MMR leaf alongside data, fitting into the existing checkpoint flow. One root, one checkpoint, one challenge mechanism.

## Persistence

- **Format**: JSON file per bucket at `{DATA_DIR}/s3_indices/bucket_{id}_index.json`
- **Writes**: Atomic (write to temp file, then rename)
- **Startup**: Scan directory, deserialize all index files
- **Env var**: `DATA_DIR` — when set, enables persistence; otherwise in-memory only

## Security Analysis

**What if the provider lies about metadata?**

The metadata Merkle root is committed to the chain via checkpoints. Clients can:
1. Request the full index and verify it hashes to the committed root
2. Challenge specific objects: request inclusion proof that a key+data_root pair is in the committed metadata root
3. Verify data integrity by downloading and checking against data_root

**What if the provider omits objects from the index?**

An exclusion proof against the committed metadata root proves a key is NOT in the index. If the client can show they previously received a PUT confirmation (signed by the provider), but the key is now missing from the committed index, this constitutes a provable violation.

## Design Decisions

1. **Single MMR** — metadata root committed as MMR leaf, not a separate structure
2. **BTreeMap** — sorted iteration for free; simple, correct, sufficient for thousands of objects per bucket
3. **JSON persistence** — human-readable, debuggable; metadata-only files are small
4. **DELETE keeps data** — removing from index doesn't GC blob storage; data remains committed in MMR for challenge proofs
5. **Server-side chunking** — PUT accepts raw bytes, chunks internally; eliminates multi-request upload dance
6. **Query param keys** — object key passed as `?key=` rather than path segment to avoid routing limitations with nested keys

## Future Considerations

- **Storage backend migration**: JSON → sled/SQLite for larger indices
- **Garbage collection**: Periodic cleanup of unreferenced blobs from deleted objects
- **Versioning**: Object version history, similar to S3 versioning
- **Multipart uploads**: For very large files (>256MB)
- **Batch operations**: Bulk PUT/DELETE for efficiency
