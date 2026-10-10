// SPDX-License-Identifier: Apache-2.0

//! The file tree against an in-process provider node, over Layer 0 routes
//! only. No chain is needed.

#[path = "../../storage/tests/common/mod.rs"]
mod common;

use common::{make_client, start_test_provider};
use file_system_client::{EmptyParents, FsClientError, Tree};
use file_system_primitives::{compute_cid, DirectoryNode, DEFAULT_MIME_TYPE};

const BUCKET: u64 = 1;

#[tokio::test]
async fn empty_bucket_has_empty_root() {
    let url = start_test_provider().await;
    let client = make_client(url);
    let tree = Tree::new(&client, BUCKET);
    let root = tree.load_root().await.unwrap();
    assert_eq!(root.cid, None);
    assert!(tree.list("/").await.unwrap().is_empty());
}

/// A second client instance finds the root through the bucket's last MMR
/// leaf and reads what the first one wrote.
#[tokio::test]
async fn fresh_client_reads_files_through_the_last_leaf() {
    let url = start_test_provider().await;
    let writer = make_client(url.clone());
    let tree = Tree::new(&writer, BUCKET);

    // Three full chunks plus a partial one.
    let large: Vec<u8> = (0..256 * 1024 * 3 + 17).map(|i| (i % 251) as u8).collect();
    tree.put_file("/docs/large.bin", &large, DEFAULT_MIME_TYPE, vec![])
        .await
        .unwrap();
    tree.put_file("/docs/hello.txt", b"hello", "text/plain", vec![])
        .await
        .unwrap();
    tree.mkdir("/empty").await.unwrap();
    tree.put_file("/empty.txt", b"", DEFAULT_MIME_TYPE, vec![])
        .await
        .unwrap();

    let reader_client = make_client(url);
    let reader = Tree::new(&reader_client, BUCKET);
    let root = reader.load_root().await.unwrap();
    assert_eq!(root.cid, Some(root.node.compute_cid()));
    let names: Vec<String> = reader
        .list("/")
        .await
        .unwrap()
        .iter()
        .map(|e| e.name_str())
        .collect();
    assert_eq!(names, ["docs", "empty", "empty.txt"]);

    let file = reader.get_file("/docs/large.bin").await.unwrap();
    assert_eq!(file.data, large);
    assert_eq!(file.stat.content_root(), compute_cid(&large));
    assert_eq!(file.stat.manifest.chunks.len(), 4);

    let hello = reader.get_file("/docs/hello.txt").await.unwrap();
    assert_eq!(hello.data, b"hello");
    assert_eq!(hello.stat.content_type(), "text/plain");

    assert!(reader.get_file("/empty.txt").await.unwrap().data.is_empty());
    assert!(reader.list("/empty").await.unwrap().is_empty());
}

/// Each write commits its blobs in one request with the new root last.
#[tokio::test]
async fn write_commits_root_as_last_leaf() {
    let url = start_test_provider().await;
    let client = make_client(url);
    let tree = Tree::new(&client, BUCKET);

    tree.put_file("/a/f", b"x", DEFAULT_MIME_TYPE, vec![])
        .await
        .unwrap();
    let commitment = client.get_commitment(BUCKET).await.unwrap();
    // content, manifest, /a, root
    assert_eq!(commitment.leaf_count, 4);
    let last = client
        .get_mmr_proof(BUCKET, commitment.leaf_count - 1)
        .await
        .unwrap();
    assert_eq!(
        Some(last.leaf.data_root),
        tree.load_root().await.unwrap().cid
    );
}

#[tokio::test]
async fn delete_and_errors_over_http() {
    let url = start_test_provider().await;
    let client = make_client(url);
    let tree = Tree::new(&client, BUCKET);

    tree.put_file("/d/f", b"x", DEFAULT_MIME_TYPE, vec![])
        .await
        .unwrap();
    assert!(matches!(
        tree.delete("/d", EmptyParents::Keep).await,
        Err(FsClientError::DirectoryNotEmpty(_))
    ));
    tree.delete("/d/f", EmptyParents::Remove).await.unwrap();
    let root = tree.load_root().await.unwrap();
    assert_eq!(root.node, DirectoryNode::new_empty(BUCKET));
    assert!(matches!(
        tree.get_file("/d/f").await,
        Err(FsClientError::PathNotFound(_))
    ));
}

/// A raw Layer 0 commit becomes the last leaf; root discovery reports it.
#[tokio::test]
async fn raw_commit_is_not_a_file_system_root() {
    let url = start_test_provider().await;
    let client = make_client(url);
    let raw = client
        .upload(BUCKET, b"not a directory", Default::default())
        .await
        .unwrap();
    client.commit(BUCKET, vec![raw]).await.unwrap();
    assert!(matches!(
        Tree::new(&client, BUCKET).load_root().await,
        Err(FsClientError::NotAFileSystemBucket { .. })
    ));
}

/// With an unknown size, a 64-byte read whose CID is an internal Merkle node is
/// rejected; a 64-byte leaf blob still reads.
#[tokio::test]
async fn sizeless_read_rejects_internal_node() {
    use file_system_client::{BlobLength, BlobStore};
    use storage_client::ChunkingStrategy;

    // Over 256 KiB, so that `/read` (which counts 256 KiB chunks) returns
    // both 32-byte chunks below.
    const MAX: u64 = 1 << 20;

    let url = start_test_provider().await;
    let client = make_client(url);

    // Two 32-byte chunks: `/read` of the root returns 64 bytes.
    let data = [7u8; 64];
    let root = client
        .upload(BUCKET, &data, ChunkingStrategy::Fixed(32))
        .await
        .unwrap();
    assert!(matches!(
        client.get_blob(root, BlobLength::AtMost(MAX)).await,
        Err(FsClientError::Serialization(e)) if e.contains("internal Merkle node")
    ));

    let leaf = client.put_blob(BUCKET, &data).await.unwrap();
    assert_eq!(leaf, compute_cid(&data));
    assert_eq!(
        client
            .get_blob(leaf, BlobLength::AtMost(MAX))
            .await
            .unwrap(),
        data
    );
}
