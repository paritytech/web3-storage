# File System Client SDK

Rust client for the Layer 1 file system on top of Scalable Web3 Storage.

## Overview

A drive is a Layer 0 bucket of `pallet-storage-provider`. The bucket id is the
drive id. The chain stores no drive name and no drive record:

- **Create**: `create_drive` submits `StorageProvider::create_bucket_with_primary`
  with provider-signed terms and returns the bucket id from the
  `BucketCreated` event.
- **Share**: `add_member` / `remove_member` submit `StorageProvider::set_member`
  / `remove_member`. There is no separate share call. The member gets
  provider access to the bucket, but cannot open the drive with this client:
  see the root CID limit below.
- **Delete**: the client has no drive deletion. Layer 0 has no bucket deletion.
- **Files and directories**: stored as content-addressed blobs (file manifests,
  chunks, directory nodes) in the bucket through the provider. The drive's
  root CID exists only in this client's memory, so only the client instance
  that created a drive can read it.

## Prerequisites

1. `just start-chain` (parachain at `ws://127.0.0.1:2222`)
2. `just start-provider` (provider at `http://127.0.0.1:3333`)
3. A registered provider that accepts primary agreements (`just demo` registers one).

## Usage

```rust
use file_system_client::{FileSystemClient, Signer};
use storage_client::{NegotiateRequest, ProviderClient, Visibility};

let mut fs_client = FileSystemClient::new(
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

let bucket_id = fs_client
    .create_drive(provider, signed.terms, signed.signature, Visibility::Private)
    .await?;

fs_client.create_directory(bucket_id, "/documents").await?;
fs_client.upload_file(bucket_id, "/documents/hello.txt", b"hello").await?;
let entries = fs_client.list_directory(bucket_id, "/documents").await?;
let data = fs_client.download_file(bucket_id, "/documents/hello.txt").await?;

// Give Bob write access to the drive.
fs_client.add_member(bucket_id, bob, file_system_client::Role::Writer).await?;
```

The signer signs both extrinsics and provider HTTP requests.

Checkpoints: `submit_checkpoint`, `submit_checkpoint_with_config`, and
`enable_auto_checkpoints` / `disable_auto_checkpoints` /
`request_immediate_checkpoint` checkpoint the drive's bucket.

## Examples

```bash
cargo run -p file-system-client --example basic_usage
just fs-demo-ci   # runs examples/ci_integration_test.rs
```

## Testing

```bash
cargo test -p file-system-client
```

## Documentation

- API reference: `cargo doc -p file-system-client -p file-system-primitives --no-deps --open`
- [Layer 0 design](../../docs/design/scalable-web3-storage.md)

## License

[Apache-2.0](../../LICENSE-APACHE2)
