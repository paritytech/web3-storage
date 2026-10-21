# S3 Client SDK

Rust client with S3-style object operations on top of Scalable Web3 Storage.

## Overview

An S3 bucket is a Layer 0 bucket of `pallet-storage-provider`, identified by
its bucket id. The chain stores no bucket name and no object metadata.

| Operation | Where it goes |
|-----------|---------------|
| `create_bucket` | Chain: `StorageProvider::create_bucket_with_primary`; returns the id from `BucketCreated` |
| `head_bucket`, `list_buckets` | Chain: `StorageProvider::Buckets`, `StorageProvider::MemberBuckets` |
| `put_object`, `delete_object` | Provider, Layer 0: `PUT /node`, `POST /commit` |
| `get_object`, `head_object`, `list_objects_v2` | Provider, Layer 0: `GET /commitment`, `GET /mmr_proof`, `GET /read`, `GET /node` |

Objects are files in the bucket's file tree, the same format the file system
client uses (`file_system_client::tree`). Key `k` is the file at path `/k`;
intermediate directories are created by `put_object` and removed by
`delete_object` when they become empty. The content type and user metadata
(keys lowercased) are stored in the file's manifest. The ETag is `0x` + hex of
the object's content root. The tree's root is the bucket's last MMR leaf, so
any client instance can read the bucket. `get_object` checks the data against
the content root.

`ObjectClient` contains the object operations and needs no chain connection;
`S3Client` adds the chain bucket operations.

Key rules: 1 to 1024 bytes, split on `/` into segments of 1 to 256 bytes, no
empty segment (no leading, trailing or double `/`), no `.` or `..` segment.

Limits:

- **Single writer.** Two clients that write the same bucket at the same time
  can lose a change: the last commit wins.
- **Only the S3 and file system clients may write the bucket.** A raw Layer 0
  commit becomes the last leaf and the client then rejects the bucket.
- **Prefix deletes.** An Admin `POST /delete` with a new `start_seq` can let
  the provider drop blobs the current tree still references.
- **Reads are unauthenticated.** Layer 0 reads need no signature: anyone who
  knows a CID can read the blob (#383, #396). Bucket visibility does not
  protect object contents. Use client-side encryption for confidential data.
- A key cannot also be a prefix directory of another key: `a` and `a/b`
  cannot both exist (`KeyConflict`).
- `list_objects_v2` reads every directory under the prefix's deepest
  directory and no manifests, so listed objects have no ETag (`head_object`
  returns it). Page with `next_start_after`.
- **No bucket deletion.** Layer 0 has no bucket deletion.
- `list_buckets` returns every bucket the account is a member of. The chain
  does not record which buckets contain S3 objects.
- All object operations go to the provider URL passed to `S3Client::new`.

## Usage

```rust
use s3_client::{PutObjectOptions, S3Client, Signer, Visibility};
use storage_client::{NegotiateRequest, ProviderClient};

let client = S3Client::new(
    "ws://127.0.0.1:2222",
    "http://127.0.0.1:3333",
    Signer::from_seed("//Alice")?,
)
.await?;

let provider = ProviderClient::fetch_provider_id("http://127.0.0.1:3333").await?;
let signed = ProviderClient::negotiate_terms(
    "http://127.0.0.1:3333",
    &NegotiateRequest {
        owner,
        max_bytes: 1_000_000_000,
        duration: 500,
        price_per_byte: 1,
        replica_params: None,
        bucket: None,
    },
)
.await?;

let bucket_id = client
    .create_bucket(provider, signed.terms, signed.signature, Visibility::Private)
    .await?;

client
    .put_object(bucket_id, "hello.txt", b"hello", PutObjectOptions::default())
    .await?;
let object = client.get_object(bucket_id, "hello.txt").await?;
let listing = client.list_objects_v2(bucket_id, Default::default()).await?;
client.delete_object(bucket_id, "hello.txt").await?;
```

## Testing

```bash
cargo test -p s3-client

# Integration example (needs a running chain and provider)
just start-chain                                  # Terminal 1
just start-provider                               # Terminal 2
cargo run -p s3-client --example basic_usage      # Terminal 3
just s3-demo-ci                                   # or the CI version
```

## Documentation

- API reference: `cargo doc -p s3-client --no-deps --open`
- [Layer 0 design](../../docs/design/scalable-web3-storage.md)

## License

[Apache-2.0](../../LICENSE-APACHE2)
