// SPDX-License-Identifier: Apache-2.0

//! File system and S3 objects against a running chain and provider: create a
//! bucket, write and read directories and files, read them from a second
//! client instance, store objects in the same tree, delete, and add and
//! remove a member.
//!
//! Usage: cargo run -p bucket-tree --example demo <chain_ws> <provider_url>
//! (`just file-system-demo`).

use bucket_tree::format::DEFAULT_MIME_TYPE;
use bucket_tree::s3::{ListObjectsParams, Objects, PutObjectOptions};
use bucket_tree::{BucketId, EmptyParents, Tree};
use sp_runtime::AccountId32;
use std::collections::HashMap;
use storage_client::{
    AdminClient, ClientConfig, NegotiateRequest, ProviderClient, Signer, StorageUserClient,
    Visibility,
};
use storage_primitives::Role;
use subxt_signer::sr25519::dev;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();
    let args: Vec<String> = std::env::args().collect();
    let [_, chain_ws, provider_url] = args.as_slice() else {
        eprintln!("Usage: {} <chain_ws> <provider_url>", args[0]);
        std::process::exit(1);
    };
    let signer = Signer::from_seed("//Alice")?;
    let (admin, bucket_id) = create_bucket(chain_ws, provider_url, signer.clone()).await?;
    assert!(admin.list_my_buckets().await?.contains(&bucket_id));

    let store = StorageUserClient::for_provider(provider_url, signer.clone())?;
    let tree = Tree::new(&store, bucket_id);

    tree.mkdir("/test-dir").await?;
    tree.mkdir("/test-dir/subdir").await?;
    let hello = b"Hello from CI integration test!";
    let nested = b"This is a nested file in the subdirectory.";
    tree.put_file("/test-dir/hello.txt", hello, "text/plain", Vec::new())
        .await?;
    tree.put_file(
        "/test-dir/subdir/nested.txt",
        nested,
        DEFAULT_MIME_TYPE,
        Vec::new(),
    )
    .await?;

    let root = tree.list("/").await?;
    assert_eq!(root.len(), 1);
    assert!(root[0].is_directory() && root[0].name_str() == "test-dir");
    assert_eq!(tree.list("/test-dir").await?.len(), 2);
    assert_eq!(tree.list("/test-dir/subdir").await?.len(), 1);
    assert_eq!(tree.get_file("/test-dir/hello.txt").await?.data, hello);
    assert_eq!(
        tree.get_file("/test-dir/subdir/nested.txt").await?.data,
        nested
    );
    println!("Wrote and read 2 directories and 2 files");

    // A second client instance finds the root through the bucket's last MMR
    // leaf.
    let second_store = StorageUserClient::for_provider(provider_url, signer)?;
    let second = Tree::new(&second_store, bucket_id);
    let file = second.get_file("/test-dir/hello.txt").await?;
    assert_eq!(file.data, hello);
    assert_eq!(file.stat.content_type(), "text/plain");

    // An empty file has a manifest and no content blob.
    tree.put_file("/empty.txt", b"", DEFAULT_MIME_TYPE, Vec::new())
        .await?;
    let empty = second.get_file("/empty.txt").await?;
    assert!(empty.data.is_empty() && empty.stat.size() == 0);
    println!(
        "Read the drive from a second client, root {:?}",
        second.load_root().await?.cid
    );

    tree.delete("/test-dir/subdir/nested.txt", EmptyParents::Keep)
        .await?;
    tree.delete("/test-dir/subdir", EmptyParents::Keep).await?;
    assert_eq!(tree.list("/test-dir").await?.len(), 1);
    println!("Deleted a file and an empty directory");

    // Objects are files in the same tree: key `k` is the file `/k`.
    let objects = Objects::new(Tree::new(&store, bucket_id));
    let payload = b"This is a binary-like payload for testing.";
    let put_hello = objects
        .put_object(
            "objects/hello.txt",
            hello,
            PutObjectOptions {
                content_type: Some("text/plain".to_string()),
                metadata: HashMap::from([("Test-Key".to_string(), "test-value".to_string())]),
            },
        )
        .await?;
    objects
        .put_object(
            "objects/data/payload.bin",
            payload,
            PutObjectOptions::default(),
        )
        .await?;

    let got = objects.get_object("objects/hello.txt").await?;
    assert_eq!(got.data, hello);
    assert_eq!(got.content_type, "text/plain");
    assert_eq!(
        got.metadata.get("test-key").map(String::as_str),
        Some("test-value")
    );
    assert_eq!(
        tree.get_file("/objects/data/payload.bin").await?.data,
        payload
    );

    let objects_prefix = ListObjectsParams {
        prefix: Some("objects/".to_string()),
        ..Default::default()
    };
    let listing = objects.list_objects_v2(objects_prefix.clone()).await?;
    let keys: Vec<&str> = listing.contents.iter().map(|o| o.key.as_str()).collect();
    assert_eq!(keys, ["objects/data/payload.bin", "objects/hello.txt"]);

    let head = objects.head_object("objects/hello.txt").await?;
    assert_eq!(head.size, hello.len() as u64);
    assert_eq!(head.cid, put_hello.cid);
    println!("Put, read, listed and headed 2 objects");

    objects.delete_object("objects/hello.txt").await?;
    objects.delete_object("objects/data/payload.bin").await?;
    assert!(objects
        .list_objects_v2(objects_prefix)
        .await?
        .contents
        .is_empty());
    assert_eq!(tree.list("/").await?.len(), 2);
    println!("Deleted both objects and their empty directories");

    let bob = AccountId32::new(dev::bob().public_key().0).to_string();
    admin
        .add_member(bucket_id, bob.clone(), Role::Writer)
        .await?;
    admin.remove_member(bucket_id, bob).await?;
    println!("Added and removed a member");

    println!("=== PASSED ===");
    Ok(())
}

/// Create a private bucket with the provider at `provider_url` as primary,
/// owned by `signer`. Returns the connected admin client and the bucket id.
async fn create_bucket(
    chain_ws: &str,
    provider_url: &str,
    signer: Signer,
) -> Result<(AdminClient, BucketId), Box<dyn std::error::Error>> {
    let owner = AccountId32::new(signer.keypair().public_key().0);
    let provider = ProviderClient::fetch_provider_id(provider_url).await?;
    let mut admin = AdminClient::new(
        ClientConfig {
            chain_ws_url: chain_ws.to_string(),
            ..Default::default()
        },
        signer,
    )?;
    admin.connect().await?;
    let nonce = admin.agreement_nonce(&owner).await?;
    let signed = ProviderClient::negotiate_terms(
        provider_url,
        &NegotiateRequest {
            owner,
            max_bytes: 1_000_000_000,
            duration: 500,
            price_per_byte: 1,
            nonce,
            bucket: None,
            replica_params: None,
        },
    )
    .await?;
    let bucket_id = admin
        .create_bucket_with_primary(provider.to_string(), signed, Visibility::Private)
        .await?;
    println!("Created bucket {bucket_id} with primary provider {provider}");
    Ok((admin, bucket_id))
}
