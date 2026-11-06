// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::format::CHUNK_SIZE;
use crate::testing::{tree, MemoryStore};

#[test]
fn key_rules() {
    assert_eq!(key_to_path("a").unwrap(), "/a");
    assert_eq!(key_to_path("a/b/c.txt").unwrap(), "/a/b/c.txt");
    assert!(key_to_path(&"a".repeat(256)).is_ok());
    let long_key = vec!["a".repeat(200); 5].join("/");
    assert_eq!(long_key.len(), 1004);
    assert!(key_to_path(&long_key).is_ok());

    let too_long = vec!["a".repeat(255); 5].join("/");
    assert!(too_long.len() > MAX_OBJECT_KEY_LEN);
    for bad in [
        "",
        "/a",
        "a/",
        "a//b",
        ".",
        "..",
        "a/./b",
        "a/../b",
        &"a".repeat(257),
        &too_long,
    ] {
        assert!(
            matches!(key_to_path(bad), Err(S3ClientError::InvalidObjectKey(_))),
            "{bad:?} must be rejected"
        );
    }
}

#[tokio::test]
async fn metadata_keys_are_lowercased_and_unique() {
    let store = MemoryStore::default();
    let objects = Objects::new(tree(&store));
    let put = |metadata: HashMap<String, String>| {
        objects.put_object(
            "k",
            b"x",
            PutObjectOptions {
                content_type: None,
                metadata,
            },
        )
    };
    put(HashMap::from([
        ("Color".to_string(), "blue".to_string()),
        ("ALPHA".to_string(), "1".to_string()),
    ]))
    .await
    .unwrap();
    assert_eq!(
        objects.head_object("k").await.unwrap().metadata,
        HashMap::from([
            ("alpha".to_string(), "1".to_string()),
            ("color".to_string(), "blue".to_string()),
        ])
    );

    let clash = HashMap::from([
        ("Color".to_string(), "blue".to_string()),
        ("color".to_string(), "red".to_string()),
    ]);
    assert!(matches!(
        put(clash).await,
        Err(S3ClientError::InvalidMetadata(_))
    ));
    let long_value = HashMap::from([("k".to_string(), "v".repeat(257))]);
    assert!(matches!(
        to_metadata_entries(long_value),
        Err(S3ClientError::InvalidMetadata(_))
    ));
    let too_many: HashMap<String, String> = (0..65)
        .map(|i| (format!("k{i}"), "v".to_string()))
        .collect();
    assert!(matches!(
        to_metadata_entries(too_many),
        Err(S3ClientError::InvalidMetadata(_))
    ));
}

const KEYS: &[&str] = &[
    "a.txt",
    "b/1.txt",
    "b/2.txt",
    "b/c/3.txt",
    "b0.txt",
    "d/4.txt",
];

fn keys_of(page: &Page<usize>) -> Vec<&'static str> {
    page.contents.iter().map(|&i| KEYS[i]).collect()
}

#[test]
fn keys_sort_by_bytes() {
    // `b/` (0x2f) sorts before `b0` (0x30): S3 order, not tree walk order.
    let mut sorted = KEYS.to_vec();
    sorted.sort();
    assert_eq!(sorted, KEYS);
}

#[test]
fn list_without_delimiter_returns_all_matching_keys() {
    let page = list_page(KEYS, "", None, None, 1000);
    assert_eq!(keys_of(&page), KEYS);
    assert!(!page.is_truncated);
    assert_eq!(page.next_start_after, None);

    let page = list_page(KEYS, "b/", None, None, 1000);
    assert_eq!(keys_of(&page), ["b/1.txt", "b/2.txt", "b/c/3.txt"]);

    let page = list_page(KEYS, "b", None, None, 1000);
    assert_eq!(
        keys_of(&page),
        ["b/1.txt", "b/2.txt", "b/c/3.txt", "b0.txt"]
    );
}

#[test]
fn list_with_delimiter_groups_common_prefixes() {
    let page = list_page(KEYS, "", Some("/"), None, 1000);
    assert_eq!(keys_of(&page), ["a.txt", "b0.txt"]);
    assert_eq!(page.common_prefixes, ["b/", "d/"]);

    let page = list_page(KEYS, "b/", Some("/"), None, 1000);
    assert_eq!(keys_of(&page), ["b/1.txt", "b/2.txt"]);
    assert_eq!(page.common_prefixes, ["b/c/"]);
}

#[test]
fn list_start_after_skips_keys_and_prefixes() {
    let page = list_page(KEYS, "", None, Some("b/2.txt"), 1000);
    assert_eq!(keys_of(&page), ["b/c/3.txt", "b0.txt", "d/4.txt"]);

    // A common prefix as start_after skips every key under it.
    let page = list_page(KEYS, "", Some("/"), Some("b/"), 1000);
    assert_eq!(keys_of(&page), ["b0.txt"]);
    assert_eq!(page.common_prefixes, ["d/"]);
}

#[test]
fn list_max_keys_truncates_and_pages() {
    let first = list_page(KEYS, "", Some("/"), None, 2);
    assert_eq!(keys_of(&first), ["a.txt"]);
    assert_eq!(first.common_prefixes, ["b/"]);
    assert!(first.is_truncated);
    assert_eq!(first.next_start_after.as_deref(), Some("b/"));

    let second = list_page(KEYS, "", Some("/"), first.next_start_after.as_deref(), 2);
    assert_eq!(keys_of(&second), ["b0.txt"]);
    assert_eq!(second.common_prefixes, ["d/"]);
    assert!(!second.is_truncated);
    assert_eq!(second.next_start_after, None);

    let exact = list_page(KEYS, "", None, None, KEYS.len());
    assert!(!exact.is_truncated);
}

#[tokio::test]
async fn list_matches_a_page_of_all_keys_with_fewer_reads() {
    use std::sync::atomic::Ordering;

    let store = MemoryStore::default();
    let t = tree(&store);
    let objects = Objects::new(tree(&store));
    let keys = [
        "a.txt",
        "b/1.txt",
        "b/c/3.txt",
        "b0.txt",
        "photos/2024/01/01/img.jpg",
        "photos/2024/01/02/img.jpg",
        "photos/2024/02/01/img.jpg",
        "photos/2025/01/01/img.jpg",
    ];
    for key in keys {
        objects
            .put_object(key, b"x", PutObjectOptions::default())
            .await
            .unwrap();
    }
    // File-system writes can leave directories without objects.
    t.mkdir("/b/empty").await.unwrap();
    t.mkdir("/e/f").await.unwrap();
    let mut all = keys.to_vec();
    all.sort();

    for prefix in [
        "",
        "b",
        "b/",
        "b/c",
        "photos/",
        "photos/2024/",
        "photos/2024/0",
        "x/",
        "a.txt/",
    ] {
        for start_after in [
            None,
            Some("a.txt"),
            Some("b/"),
            Some("b/1.txt"),
            Some("b/c/3.txt"),
            Some("photos/2024/"),
            Some("photos/2024/01/01/img.jpg"),
            Some("photos/2024/01/02/img.jpg"),
            Some("zzz"),
        ] {
            for (delimiter, max_keys) in [None, Some("/"), Some("0")]
                .into_iter()
                .flat_map(|d| [1, 2, 1000].map(|max| (d, max)))
            {
                let listed = objects
                    .list_objects(ListObjectsParams {
                        prefix: Some(prefix.to_string()),
                        delimiter: delimiter.map(str::to_string),
                        start_after: start_after.map(str::to_string),
                        max_keys: Some(max_keys),
                    })
                    .await
                    .unwrap();
                let walked = list_page(&all, prefix, delimiter, start_after, max_keys as usize);
                let context =
                    format!("{prefix:?} {delimiter:?} after {start_after:?} max {max_keys}");
                let listed_keys: Vec<&str> =
                    listed.contents.iter().map(|o| o.key.as_str()).collect();
                let walked_keys: Vec<&str> = walked.contents.iter().map(|&i| all[i]).collect();
                assert_eq!(listed_keys, walked_keys, "{context}");
                assert_eq!(listed.common_prefixes, walked.common_prefixes, "{context}");
                assert_eq!(listed.is_truncated, walked.is_truncated, "{context}");
                assert_eq!(
                    listed.next_start_after, walked.next_start_after,
                    "{context}"
                );
            }
        }
    }

    // The root, `b`, `e` and `e/f`, and one path down `photos` to a
    // file: 8 of the 15 directories.
    store.reads.store(0, Ordering::Relaxed);
    let listed = objects
        .list_objects(ListObjectsParams {
            delimiter: Some("/".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(listed.common_prefixes, ["b/", "photos/"]);
    assert_eq!(store.reads.load(Ordering::Relaxed), 8);
}

const BIG_SIZE: usize = 3 * CHUNK_SIZE + 100;

/// Root CID of the bucket that `write_sequence` produces. It covers every
/// directory node, manifest and content root, so a change to the bucket
/// format changes it. Clients cannot read buckets written in an older
/// format, so update it only with a reviewed format change.
const ROOT_CID: &str = "0x73c28efba4f06b593cc13f1f6aa59f1bf9c6fd0a4a1d5908e5b2213989f130c7";

/// A fixed sequence of file and object writes, with a fixed clock.
async fn write_sequence(store: &MemoryStore) {
    let t = tree(store);
    t.mkdir("/docs").await.unwrap();
    t.put_file("/docs/a.txt", b"hello from a.txt\n", "text/plain", vec![])
        .await
        .unwrap();
    t.put_file("/docs/old.txt", b"deleted later", "text/plain", vec![])
        .await
        .unwrap();
    let big: Vec<u8> = (0..BIG_SIZE).map(|i| (i % 251) as u8).collect();
    t.put_file("/big.bin", &big, DEFAULT_MIME_TYPE, vec![])
        .await
        .unwrap();
    t.put_file("/empty", b"", DEFAULT_MIME_TYPE, vec![])
        .await
        .unwrap();
    Objects::new(tree(store))
        .put_object(
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

#[tokio::test]
async fn the_bucket_format_produces_the_pinned_root() {
    let store = MemoryStore::default();
    write_sequence(&store).await;
    let root = tree(&store).load_root().await.unwrap().cid.unwrap();
    assert_eq!(format!("{root:#x}"), ROOT_CID);
}

/// A listing stops reading once its page is full.
#[tokio::test]
async fn a_small_page_reads_few_directories() {
    use std::sync::atomic::Ordering;

    let store = MemoryStore::default();
    let objects = Objects::new(tree(&store));
    for dir in 'a'..='z' {
        objects
            .put_object(&format!("{dir}/1"), b"x", PutObjectOptions::default())
            .await
            .unwrap();
    }
    store.reads.store(0, Ordering::Relaxed);
    let page = objects
        .list_objects(ListObjectsParams {
            max_keys: Some(1),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(page.next_start_after.as_deref(), Some("a/1"));
    // The root, `a`, and `b`, whose first key shows the page is full.
    assert_eq!(store.reads.load(Ordering::Relaxed), 3);
}
