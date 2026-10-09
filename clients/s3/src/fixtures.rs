// SPDX-License-Identifier: Apache-2.0

//! Cross-language fixtures for the in-bucket file-system format.
//!
//! `rust_written_fixture_is_current` runs a fixed sequence of FS and S3
//! writes into an in-memory store and checks the result against
//! `crates/primitives/file-system/fixtures/rust-written.json`, which the
//! TypeScript tests (`packages/layer1/src/fixtures.test.ts`) load and read.
//! `reads_the_typescript_fixture` does the reverse with `ts-written.json`.
//!
//! Regenerate a fixture with `UPDATE_FS_FIXTURES=1`, here with
//! `UPDATE_FS_FIXTURES=1 cargo test -p s3-client fixtures`.
//!
//! Fixture format:
//! - `blobs`: CID -> base64 bytes, for every blob of at most one chunk.
//! - `generated_blobs`: blobs larger than one chunk (only `/big.bin`), as the
//!   generator rule, size, CID and chunk hashes, to keep the file small.
//! - `commits`: the `data_roots` of each `POST /commit`, in order.
//! - `expected`: root CID, listings, file and object reads.

use super::*;
use base64::Engine as _;
use file_system_client::BlobLength;
use file_system_primitives::{chunk_hashes, compute_cid, Cid, CHUNK_SIZE};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::sync::Mutex;

const BUCKET: BucketId = 7;
const MTIME: u64 = 1_700_000_000;
const GENERATOR: &str = "byte i is i % 251";
const BIG_SIZE: usize = 3 * CHUNK_SIZE + 100;
const UPDATE_ENV: &str = "UPDATE_FS_FIXTURES";

fn fixture_path(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../crates/primitives/file-system/fixtures")
        .join(name)
}

fn generate(size: usize) -> Vec<u8> {
    (0..size).map(|i| (i % 251) as u8).collect()
}

fn hex_cid(cid: &Cid) -> String {
    format!("0x{}", hex::encode(cid.as_bytes()))
}

fn parse_cid(s: &str) -> Cid {
    let bytes = hex::decode(s.strip_prefix("0x").unwrap()).unwrap();
    Cid::from_slice(&bytes)
}

/// In-memory Layer 0: blobs by CID and the committed `data_roots`.
#[derive(Default)]
struct MemoryStore {
    blobs: Mutex<BTreeMap<Cid, Vec<u8>>>,
    commits: Mutex<Vec<Vec<Cid>>>,
}

#[async_trait::async_trait]
impl BlobStore for MemoryStore {
    async fn put_blob(&self, _bucket_id: BucketId, data: &[u8]) -> file_system_client::Result<Cid> {
        let cid = compute_cid(data);
        self.blobs.lock().unwrap().insert(cid, data.to_vec());
        Ok(cid)
    }

    async fn get_blob(&self, cid: Cid, _length: BlobLength) -> file_system_client::Result<Vec<u8>> {
        self.blobs
            .lock()
            .unwrap()
            .get(&cid)
            .cloned()
            .ok_or_else(|| FsClientError::StorageClient(format!("missing blob {cid:?}")))
    }

    async fn commit(
        &self,
        _bucket_id: BucketId,
        data_roots: Vec<Cid>,
    ) -> file_system_client::Result<()> {
        let blobs = self.blobs.lock().unwrap();
        assert!(data_roots.iter().all(|c| blobs.contains_key(c)));
        self.commits.lock().unwrap().push(data_roots);
        Ok(())
    }

    async fn last_leaf(&self, _bucket_id: BucketId) -> file_system_client::Result<Option<Cid>> {
        Ok(self
            .commits
            .lock()
            .unwrap()
            .last()
            .and_then(|c| c.last().copied()))
    }
}

fn tree(store: &MemoryStore) -> Tree<&MemoryStore> {
    Tree::with_clock(store, BUCKET, || MTIME)
}

/// The write sequence. The TypeScript fixture test runs the same one.
async fn write_sequence(store: &MemoryStore) {
    let t = tree(store);
    t.mkdir("/docs").await.unwrap();
    t.put_file("/docs/a.txt", b"hello from a.txt\n", "text/plain", vec![])
        .await
        .unwrap();
    t.put_file("/docs/old.txt", b"deleted later", "text/plain", vec![])
        .await
        .unwrap();
    t.put_file("/big.bin", &generate(BIG_SIZE), DEFAULT_MIME_TYPE, vec![])
        .await
        .unwrap();
    t.put_file("/empty", b"", DEFAULT_MIME_TYPE, vec![])
        .await
        .unwrap();
    put_object_in(
        &t,
        "photos/cat.jpg",
        b"meow",
        PutObjectOptions {
            content_type: Some("image/jpeg".into()),
            metadata: HashMap::from([
                ("Zeta".into(), "last".into()),
                ("alpha".into(), "first".into()),
                ("Camera-Model".into(), "X100".into()),
            ]),
        },
    )
    .await
    .unwrap();
    t.delete("/docs/old.txt", EmptyParents::Keep).await.unwrap();
}

/// What a reader of the bucket sees.
async fn expected(store: &MemoryStore) -> Value {
    let t = tree(store);
    let root = t.load_root().await.unwrap();

    let mut listings = Map::new();
    for dir in ["/", "/docs", "/photos"] {
        let entries: Vec<Value> = t
            .list(dir)
            .await
            .unwrap()
            .iter()
            .map(|e| {
                json!({
                    "name": e.name_str(),
                    "entry_type": if e.is_directory() { "directory" } else { "file" },
                    "cid": hex_cid(&e.cid),
                    "size": e.size,
                    "mtime": e.mtime,
                })
            })
            .collect();
        listings.insert(dir.into(), entries.into());
    }

    let mut files = Map::new();
    for path in ["/big.bin", "/docs/a.txt", "/empty", "/photos/cat.jpg"] {
        let file = t.get_file(path).await.unwrap();
        let stat = &file.stat;
        let metadata: Vec<Value> = stat
            .manifest
            .user_metadata
            .iter()
            .map(|m| {
                json!([
                    String::from_utf8(m.key.to_vec()).unwrap(),
                    String::from_utf8(m.value.to_vec()).unwrap()
                ])
            })
            .collect();
        files.insert(
            path.into(),
            json!({
                "size": stat.size(),
                "content_type": stat.content_type(),
                "blake2_256": hex_cid(&storage_primitives::blake2_256(&file.data)),
                "content_root": hex_cid(&stat.content_root()),
                "manifest_cid": hex_cid(&stat.entry.cid),
                "mtime": stat.mtime(),
                "user_metadata": metadata,
            }),
        );
    }

    let object = get_object_in(&t, "photos/cat.jpg").await.unwrap();
    let head = head_object_in(&t, "photos/cat.jpg").await.unwrap();
    assert_eq!(head.etag, object.etag);
    assert_eq!(head.metadata, object.metadata);
    let metadata: BTreeMap<_, _> = object.metadata.into_iter().collect();
    let objects = json!({
        "photos/cat.jpg": {
            "etag": object.etag,
            "content_type": object.content_type,
            "size": object.size,
            "last_modified": object.last_modified,
            "metadata": metadata,
            "blake2_256": hex_cid(&storage_primitives::blake2_256(&object.data)),
        }
    });

    json!({
        "root_cid": hex_cid(&root.cid.unwrap()),
        "listings": listings,
        "files": files,
        "objects": objects,
    })
}

/// The fixture JSON for the bucket in `store`.
async fn fixture(store: &MemoryStore, about: &str) -> String {
    let mut blobs = Map::new();
    let mut generated = Vec::new();
    for (cid, bytes) in store.blobs.lock().unwrap().iter() {
        if bytes.len() > CHUNK_SIZE {
            assert_eq!(
                *bytes,
                generate(bytes.len()),
                "only generated blobs span chunks"
            );
            let hashes: Vec<String> = chunk_hashes(bytes).iter().map(hex_cid).collect();
            generated.push(json!({
                "cid": hex_cid(cid),
                "size": bytes.len(),
                "generator": GENERATOR,
                "chunk_hashes": hashes,
            }));
        } else {
            blobs.insert(
                hex_cid(cid),
                base64::engine::general_purpose::STANDARD
                    .encode(bytes)
                    .into(),
            );
        }
    }
    let commits: Vec<Vec<String>> = store
        .commits
        .lock()
        .unwrap()
        .iter()
        .map(|c| c.iter().map(hex_cid).collect())
        .collect();
    let value = json!({
        "about": about,
        "bucket_id": BUCKET,
        "mtime": MTIME,
        "blobs": blobs,
        "generated_blobs": generated,
        "commits": commits,
        "expected": expected(store).await,
    });
    serde_json::to_string_pretty(&value).unwrap() + "\n"
}

/// Load a fixture's blobs and commits into a new store. Checks each blob's
/// CID and each generated blob's chunk hashes.
fn load(fixture: &Value) -> MemoryStore {
    assert_eq!(fixture["bucket_id"], BUCKET);
    let store = MemoryStore::default();
    {
        let mut blobs = store.blobs.lock().unwrap();
        for (cid, b64) in fixture["blobs"].as_object().unwrap() {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(b64.as_str().unwrap())
                .unwrap();
            assert_eq!(hex_cid(&compute_cid(&bytes)), *cid);
            blobs.insert(parse_cid(cid), bytes);
        }
        for blob in fixture["generated_blobs"].as_array().unwrap() {
            assert_eq!(blob["generator"], GENERATOR);
            let bytes = generate(blob["size"].as_u64().unwrap() as usize);
            let hashes: Vec<Value> = chunk_hashes(&bytes)
                .iter()
                .map(|h| hex_cid(h).into())
                .collect();
            assert_eq!(blob["chunk_hashes"], Value::Array(hashes));
            assert_eq!(blob["cid"], hex_cid(&compute_cid(&bytes)));
            blobs.insert(parse_cid(blob["cid"].as_str().unwrap()), bytes);
        }
    }
    *store.commits.lock().unwrap() = fixture["commits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            c.as_array()
                .unwrap()
                .iter()
                .map(|cid| parse_cid(cid.as_str().unwrap()))
                .collect()
        })
        .collect();
    store
}

const RUST_ABOUT: &str = "Written by clients/s3/src/fixtures.rs; read by packages/layer1/src/fixtures.test.ts. Regenerate: UPDATE_FS_FIXTURES=1 cargo test -p s3-client fixtures";

#[tokio::test]
async fn rust_written_fixture_is_current() {
    let store = MemoryStore::default();
    write_sequence(&store).await;
    let produced = fixture(&store, RUST_ABOUT).await;

    // The fixture loads back and reads the same.
    let parsed: Value = serde_json::from_str(&produced).unwrap();
    assert_eq!(expected(&load(&parsed)).await, parsed["expected"]);

    let path = fixture_path("rust-written.json");
    if std::env::var_os(UPDATE_ENV).is_some() {
        std::fs::write(&path, &produced).unwrap();
    }
    let checked_in = std::fs::read_to_string(&path).unwrap();
    assert!(
        checked_in == produced,
        "{} is out of date; regenerate it with {UPDATE_ENV}=1",
        path.display()
    );
}

#[tokio::test]
async fn reads_the_typescript_fixture() {
    let text = std::fs::read_to_string(fixture_path("ts-written.json")).unwrap();
    let fixture: Value = serde_json::from_str(&text).unwrap();
    let store = load(&fixture);

    // Root discovery: the root is the last root of the last commit.
    let last = fixture["commits"].as_array().unwrap().last().unwrap();
    let root = tree(&store).load_root().await.unwrap();
    assert_eq!(
        Value::from(hex_cid(&root.cid.unwrap())),
        *last.as_array().unwrap().last().unwrap()
    );
    assert_eq!(
        fixture["expected"]["root_cid"],
        *last.as_array().unwrap().last().unwrap()
    );

    assert_eq!(expected(&store).await, fixture["expected"]);

    // Both languages write the same bucket for the same operations.
    let rust = MemoryStore::default();
    write_sequence(&rust).await;
    assert_eq!(expected(&rust).await, fixture["expected"]);
    assert_eq!(
        *rust.commits.lock().unwrap(),
        *store.commits.lock().unwrap()
    );
}
