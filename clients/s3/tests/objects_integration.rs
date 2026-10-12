// SPDX-License-Identifier: Apache-2.0

//! S3 object operations against an in-process provider node, over Layer 0
//! routes only. No chain is needed.

#[path = "../../storage/tests/common/mod.rs"]
mod common;

use common::start_test_provider;
use s3_client::{ListObjectsParams, ObjectClient, PutObjectOptions, S3ClientError};
use std::collections::HashMap;

const BUCKET: u64 = 1;

fn client(url: &str) -> ObjectClient {
    ObjectClient::new(url, subxt_signer::sr25519::dev::alice().into()).unwrap()
}

#[tokio::test]
async fn put_then_get_from_a_fresh_client() {
    let url = start_test_provider().await;
    let writer = client(&url);
    let put = writer
        .put_object(
            BUCKET,
            "photos/2024/cat.jpg",
            b"meow",
            PutObjectOptions {
                content_type: Some("image/jpeg".into()),
                metadata: HashMap::from([("Camera".into(), "x100".into())]),
            },
        )
        .await
        .unwrap();
    assert_eq!(put.size, 4);
    assert_eq!(put.etag, format!("0x{}", hex::encode(put.cid.as_bytes())));

    let reader = client(&url);
    let object = reader
        .get_object(BUCKET, "photos/2024/cat.jpg")
        .await
        .unwrap();
    assert_eq!(object.data, b"meow");
    assert_eq!(object.content_type, "image/jpeg");
    assert_eq!(object.etag, put.etag);
    assert_eq!(
        object.metadata.get("camera").map(String::as_str),
        Some("x100")
    );

    let head = reader
        .head_object(BUCKET, "photos/2024/cat.jpg")
        .await
        .unwrap();
    assert_eq!(head.cid, put.cid);
    assert_eq!(head.size, 4);
}

#[tokio::test]
async fn empty_content_type_is_the_default() {
    let url = start_test_provider().await;
    let c = client(&url);
    c.put_object(
        BUCKET,
        "a",
        b"x",
        PutObjectOptions {
            content_type: Some(String::new()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let default = "application/octet-stream";
    assert_eq!(
        c.head_object(BUCKET, "a").await.unwrap().content_type,
        default
    );
    assert_eq!(
        c.get_object(BUCKET, "a").await.unwrap().content_type,
        default
    );
}

#[tokio::test]
async fn missing_keys_and_conflicts() {
    let url = start_test_provider().await;
    let c = client(&url);
    assert!(matches!(
        c.get_object(BUCKET, "nope").await,
        Err(S3ClientError::ObjectNotFound { .. })
    ));
    c.delete_object(BUCKET, "nope").await.unwrap();

    c.put_object(BUCKET, "a/b", b"1", Default::default())
        .await
        .unwrap();
    assert!(matches!(
        c.head_object(BUCKET, "a").await,
        Err(S3ClientError::ObjectNotFound { .. })
    ));
    assert!(matches!(
        c.put_object(BUCKET, "a", b"2", Default::default()).await,
        Err(S3ClientError::KeyConflict(_))
    ));
    assert!(matches!(
        c.put_object(BUCKET, "a/b/c", b"2", Default::default())
            .await,
        Err(S3ClientError::KeyConflict(_))
    ));
    assert!(matches!(
        c.put_object(BUCKET, "a//b", b"2", Default::default()).await,
        Err(S3ClientError::InvalidObjectKey(_))
    ));
}

#[tokio::test]
async fn list_and_delete() {
    let url = start_test_provider().await;
    let c = client(&url);
    for key in ["b0.txt", "b/2.txt", "a.txt", "b/c/3.txt", "b/1.txt"] {
        c.put_object(BUCKET, key, key.as_bytes(), Default::default())
            .await
            .unwrap();
    }

    let all = c
        .list_objects_v2(BUCKET, ListObjectsParams::default())
        .await
        .unwrap();
    let keys: Vec<&str> = all.contents.iter().map(|o| o.key.as_str()).collect();
    assert_eq!(keys, ["a.txt", "b/1.txt", "b/2.txt", "b/c/3.txt", "b0.txt"]);
    assert_eq!(all.key_count, 5);
    assert_eq!(all.contents[1].etag, None);
    assert_eq!(all.contents[1].size, 7);

    let page = c
        .list_objects_v2(
            BUCKET,
            ListObjectsParams {
                prefix: Some("b/".into()),
                delimiter: Some("/".into()),
                max_keys: Some(2),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let keys: Vec<&str> = page.contents.iter().map(|o| o.key.as_str()).collect();
    assert_eq!(keys, ["b/1.txt", "b/2.txt"]);
    assert!(page.common_prefixes.is_empty());
    assert!(page.is_truncated);
    assert_eq!(page.next_start_after.as_deref(), Some("b/2.txt"));

    // `max_keys: 0` counts as 1, so the page names where to continue.
    let one = c
        .list_objects_v2(
            BUCKET,
            ListObjectsParams {
                max_keys: Some(0),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(one.key_count, 1);
    assert!(one.is_truncated);
    assert_eq!(one.next_start_after.as_deref(), Some("a.txt"));

    let empty = c
        .list_objects_v2(
            BUCKET,
            ListObjectsParams {
                prefix: Some("zzz/".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(empty.contents.is_empty());

    // Deleting the last key under `b/c/` removes the `b/c` directory.
    c.delete_object(BUCKET, "b/c/3.txt").await.unwrap();
    let top = c
        .list_objects_v2(
            BUCKET,
            ListObjectsParams {
                prefix: Some("b/".into()),
                delimiter: Some("/".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(top.common_prefixes.is_empty());
    assert_eq!(top.key_count, 2);
}
