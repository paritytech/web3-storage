// SPDX-License-Identifier: Apache-2.0

mod fixtures;

use super::*;

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

#[test]
fn metadata_keys_are_lowercased_sorted_and_unique() {
    let metadata = HashMap::from([
        ("Color".to_string(), "blue".to_string()),
        ("ALPHA".to_string(), "1".to_string()),
    ]);
    let entries = to_metadata_entries(metadata).unwrap();
    let keys: Vec<&[u8]> = entries.iter().map(|e| e.key.as_slice()).collect();
    assert_eq!(keys, [&b"alpha"[..], b"color"]);

    let clash = HashMap::from([
        ("Color".to_string(), "blue".to_string()),
        ("color".to_string(), "red".to_string()),
    ]);
    assert!(matches!(
        to_metadata_entries(clash),
        Err(S3ClientError::InvalidMetadata(_))
    ));
    let long_value = HashMap::from([("k".to_string(), "v".repeat(257))]);
    assert!(to_metadata_entries(long_value).is_err());
}

const KEYS: &[&str] = &[
    "a.txt",
    "b/1.txt",
    "b/2.txt",
    "b/c/3.txt",
    "b0.txt",
    "d/4.txt",
];

fn keys_of(page: &Page) -> Vec<&'static str> {
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
async fn slash_delimited_list_matches_a_full_walk_with_fewer_reads() {
    use std::sync::atomic::Ordering;

    let store = fixtures::MemoryStore::default();
    let t = fixtures::tree(&store);
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
        put_object_in(&t, key, b"x", PutObjectOptions::default())
            .await
            .unwrap();
    }
    // File-system writes can leave directories without objects.
    t.mkdir("/b/empty").await.unwrap();
    t.mkdir("/e/f").await.unwrap();
    let mut all: Vec<String> = t
        .files_under("/")
        .await
        .unwrap()
        .into_iter()
        .map(|(key, _)| key)
        .collect();
    all.sort();
    let all: Vec<&str> = all.iter().map(String::as_str).collect();

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
            for max_keys in [1, 2, 1000] {
                let listed = list_objects_in(
                    &t,
                    ListObjectsParams {
                        prefix: Some(prefix.to_string()),
                        delimiter: Some("/".to_string()),
                        start_after: start_after.map(str::to_string),
                        max_keys: Some(max_keys),
                    },
                )
                .await
                .unwrap();
                let walked = list_page(&all, prefix, Some("/"), start_after, max_keys as usize);
                let context = format!("{prefix:?} after {start_after:?} max {max_keys}");
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
    let listed = list_objects_in(
        &t,
        ListObjectsParams {
            delimiter: Some("/".to_string()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(listed.common_prefixes, ["b/", "photos/"]);
    assert_eq!(store.reads.load(Ordering::Relaxed), 8);
}
