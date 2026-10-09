# S3 Client SDK

Rust client with S3-style object operations on top of Scalable Web3 Storage.

## Overview

An S3 bucket is a Layer 0 bucket of `pallet-storage-provider`, identified by
its bucket id. The chain stores no bucket name and no object metadata.

| Operation | Where it goes |
|-----------|---------------|
| `create_bucket` | Chain: `StorageProvider::create_bucket_with_primary`; returns the id from `BucketCreated` |
| `head_bucket`, `list_buckets` | Chain: `StorageProvider::Buckets`, `StorageProvider::MemberBuckets` |
| `put_object` | Provider: `PUT /s3/{bucket_id}/object?key=...` |
| `get_object` | Provider: `GET /s3/{bucket_id}/object?key=...` |
| `head_object` | Provider: `HEAD /s3/{bucket_id}/object?key=...` |
| `delete_object` | Provider: `DELETE /s3/{bucket_id}/object?key=...` |
| `list_objects_v2` | Provider: `GET /s3/{bucket_id}/objects` |

The provider chunks the uploaded bytes, commits them to the bucket, and keeps
an S3 index that maps each key to the data root, content type and user
metadata (`x-amz-meta-*` headers). Every provider request carries a signed
`Authorization` header; the provider checks the signer's bucket role.

Limits:

- **Downloads by key are unverified.** Nothing on chain commits to the
  provider's key-to-content index, so the client cannot check that the
  returned bytes belong to the key (#410).
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
