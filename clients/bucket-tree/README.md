# File System Client

Rust library for a file system and S3-style objects stored in a Scalable Web3
Storage bucket.

A drive is a Layer 0 bucket of `pallet-storage-provider`, identified by its
bucket id. Directories (`DirectoryNode`), file manifests (`FileManifest`) and
file contents are blobs in the bucket, and the root directory is the bucket's
last MMR leaf, so any client instance can open the drive. The crate talks to
the provider's Layer 0 routes only and makes no chain calls.

- `format`: the blob types (`DirectoryNode`, `FileManifest`) and their rules.
- `tree`: root discovery, the write order and the limits (single writer,
  unauthenticated blob reads, prefix deletes). Read its module docs before
  writing to a bucket.
- `s3`: S3-style objects on the same tree. Key `k` is the file at path `/k`.

Create the bucket and manage its members with `storage_client::AdminClient`,
and checkpoint it with `storage_client::CheckpointManager`.

## Usage

```rust
use bucket_tree::{s3::Objects, EmptyParents, Tree};
use storage_client::{Signer, StorageUserClient};

let store = StorageUserClient::for_provider("http://127.0.0.1:3333", Signer::from_seed("//Alice")?)?;

let tree = Tree::new(&store, bucket_id);
tree.mkdir("/documents").await?;
tree.put_file("/documents/hello.txt", b"hello", "text/plain", Vec::new()).await?;
let entries = tree.list("/documents").await?;
let file = tree.get_file("/documents/hello.txt").await?;
tree.delete("/documents/hello.txt", EmptyParents::Keep).await?;

let objects = Objects::new(Tree::new(&store, bucket_id));
objects.put_object("photos/cat.jpg", &jpeg, Default::default()).await?;
let listing = objects.list_objects_v2(Default::default()).await?;
```

`create_bucket` in `examples/demo.rs` shows how to create the bucket: negotiate signed terms
with the provider (`ProviderClient::negotiate_terms`), then
`AdminClient::create_bucket_with_primary`.

## Examples and tests

```bash
cargo test -p bucket-tree
just file-system-demo   # examples/demo.rs, needs a running chain and provider
```

## Documentation

- API reference: `cargo doc -p bucket-tree --no-deps --open`
- [Layer 0 design](../../docs/design/scalable-web3-storage.md)

## License

[Apache-2.0](../../LICENSE-APACHE2)
