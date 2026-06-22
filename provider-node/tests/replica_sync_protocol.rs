// SPDX-License-Identifier: GPL-3.0-only

//! Integration tests for the L1/L2 replica sync protocol.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use storage_primitives::{blake2_256, BucketId};
use storage_provider_node::{
    build_padded_merkle_tree, create_router, ProviderState, ReplicaSync, Storage, StorageBackend,
};

// Boot a server with a signing keypair (needed for push receipts).
async fn start_server_with_key(storage: Arc<Storage>, seed: &str) -> String {
    let state = ProviderState::with_seed(storage, seed).expect("valid seed");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = create_router(Arc::new(state));
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let client = reqwest::Client::new();
    for _ in 0..50 {
        if client
            .get(format!("http://{addr}/health"))
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    format!("http://{addr}")
}
use tokio::net::TcpListener;

const BUCKET: BucketId = 1;

async fn start_server(storage: Arc<Storage>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = create_router(Arc::new(ProviderState::new(storage, "test".into())));
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    // Wait for the server to be ready.
    let client = reqwest::Client::new();
    for _ in 0..50 {
        if client
            .get(format!("http://{addr}/health"))
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    format!("http://{addr}")
}

/// Seed `n` leaves (each a single chunk) into storage and commit them.
fn seed(storage: &Arc<Storage>, n: u64) {
    storage.init_bucket(BUCKET, u64::MAX);
    for i in 0..n {
        let data = format!("leaf-{i}").into_bytes();
        let hash = blake2_256(&data);
        storage.store_node(BUCKET, hash, data, None).unwrap();
        let root = build_padded_merkle_tree(storage.as_ref(), BUCKET, &[hash]);
        storage.commit(BUCKET, vec![root]).unwrap();
    }
}

// ── L1: offer/want exchange ───────────────────────────────────────────────────

#[tokio::test]
async fn test_l1_offer_want_full_sync() {
    let a = Arc::new(Storage::new());
    seed(&a, 5);
    let a_url = start_server(a.clone()).await;

    let b = Arc::new(Storage::new());
    let replica = ReplicaSync::new(b.clone());

    let synced = replica.sync_from_primary(BUCKET, &a_url).await.unwrap();
    let a_root = a.get_bucket(BUCKET).unwrap().mmr_root;
    assert_eq!(synced, a_root, "B must converge to A's root");
    assert_eq!(b.total_nodes(), a.total_nodes(), "all nodes transferred");
}

#[tokio::test]
async fn test_l1_offer_only_diff_moves() {
    // A has 5 leaves; B already has 3. Only 2 leaves should move.
    let a = Arc::new(Storage::new());
    seed(&a, 5);
    let a_url = start_server(a.clone()).await;

    let b = Arc::new(Storage::new());
    seed(&b, 3);
    let before = b.total_nodes();
    let replica = ReplicaSync::new(b.clone());
    replica.sync_from_primary(BUCKET, &a_url).await.unwrap();

    let transferred = b.total_nodes() - before;
    // Each single-chunk leaf = 1 node. 2 new leaves → 2 new nodes.
    assert_eq!(
        transferred, 2,
        "only the 2 missing leaves should be transferred"
    );
}

#[tokio::test]
async fn test_l1_no_data_moved_when_already_synced() {
    let a = Arc::new(Storage::new());
    seed(&a, 4);
    let a_url = start_server(a.clone()).await;

    let b = Arc::new(Storage::new());
    let replica = ReplicaSync::new(b.clone());
    replica.sync_from_primary(BUCKET, &a_url).await.unwrap();

    // Sync again — nothing should change.
    let before = b.total_nodes();
    replica.sync_from_primary(BUCKET, &a_url).await.unwrap();
    assert_eq!(b.total_nodes(), before, "second sync must be a no-op");
}

// ── L2: interval store + epoch fingerprint ────────────────────────────────────

#[tokio::test]
async fn test_l2_interval_store_records_synced_ranges() {
    let a = Arc::new(Storage::new());
    seed(&a, 6);
    let a_url = start_server(a.clone()).await;

    let b = Arc::new(Storage::new());
    let replica = ReplicaSync::new(b.clone());
    replica.sync_from_primary(BUCKET, &a_url).await.unwrap();

    let ranges = replica.synced_ranges(&a_url, BUCKET);
    assert!(
        !ranges.is_empty(),
        "interval store must record at least one range"
    );
    // The range must cover leaves 0..6.
    let (start, end) = ranges[0];
    assert_eq!(start, 0);
    assert_eq!(end, 6);
}

#[tokio::test]
async fn test_l2_delta_adds_to_interval_store() {
    let a = Arc::new(Storage::new());
    seed(&a, 4);
    let a_url = start_server(a.clone()).await;

    let b = Arc::new(Storage::new());
    let replica = ReplicaSync::new(b.clone());
    replica.sync_from_primary(BUCKET, &a_url).await.unwrap();

    // A grows; B syncs the delta.
    seed(&a, 2); // adds leaves 4 and 5
    replica.sync_from_primary(BUCKET, &a_url).await.unwrap();

    let ranges = replica.synced_ranges(&a_url, BUCKET);
    // Both sync passes recorded; total coverage is 0..6.
    let covered: u64 = ranges.iter().map(|(s, e)| e - s).sum();
    assert_eq!(covered, 6, "combined ranges must cover all 6 leaves");
}

// ── L3 pull: bidirectional anti-entropy ───────────────────────────────────────

#[tokio::test]
async fn test_l3_bidirectional_pull_convergence() {
    // Scenario: P is the primary with 10 leaves.
    // R1 syncs from P first (gets all 10). P then adds 5 more leaves.
    // R2 syncs from R1 (not P) to get the original 10.
    // Then R2 syncs from P to get the remaining 5.
    // Finally R1 can sync from R2 after R2 is ahead — shows both directions used.
    //
    // More directly: P grows to 15, R2 syncs from R1, R1 syncs from P.
    // Then P grows to 20, R1 syncs from R2 (if R2 has already synced 15..20).

    // P: primary with 10 leaves.
    let p = Arc::new(Storage::new());
    seed(&p, 10);
    let p_url = start_server(p.clone()).await;

    // R1: syncs all 10 from P.
    let r1 = Arc::new(Storage::new());
    let r1_sync = ReplicaSync::new(r1.clone());
    r1_sync.sync_from_primary(BUCKET, &p_url).await.unwrap();
    assert_eq!(r1.get_bucket(BUCKET).unwrap().leaf_count, 10);

    let r1_url = start_server(r1.clone()).await;

    // R2: syncs all 10 from R1, NOT from P — anti-entropy: R2 gets data from a peer.
    let r2 = Arc::new(Storage::new());
    let r2_sync = ReplicaSync::new(r2.clone());
    r2_sync.sync_from_primary(BUCKET, &r1_url).await.unwrap();
    assert_eq!(r2.get_bucket(BUCKET).unwrap().leaf_count, 10);
    assert_eq!(
        r1.get_bucket(BUCKET).unwrap().mmr_root,
        r2.get_bucket(BUCKET).unwrap().mmr_root,
        "R1 and R2 must converge after R2 syncs from R1"
    );

    let r2_url = start_server(r2.clone()).await;

    // P adds 5 more leaves; R2 syncs them directly from P.
    seed(&p, 5);
    r2_sync.sync_from_primary(BUCKET, &p_url).await.unwrap();
    assert_eq!(r2.get_bucket(BUCKET).unwrap().leaf_count, 15);

    // R1 syncs from R2 (not P) — the other direction: R1 gets new leaves from R2.
    r1_sync.sync_from_primary(BUCKET, &r2_url).await.unwrap();
    assert_eq!(r1.get_bucket(BUCKET).unwrap().leaf_count, 15);
    assert_eq!(
        r1.get_bucket(BUCKET).unwrap().mmr_root,
        r2.get_bucket(BUCKET).unwrap().mmr_root,
        "R1 and R2 must converge after R1 syncs from R2"
    );
}

// ── L3 push: signed custody receipts ─────────────────────────────────────────

#[tokio::test]
async fn test_l3_push_and_receipt_verification() {
    // Writer (with keypair) pushes data to peer; peer returns signed receipt.
    let writer_storage = Arc::new(Storage::new());
    seed(&writer_storage, 3);
    let committed_root = writer_storage.get_bucket(BUCKET).unwrap().mmr_root;

    let peer_storage = Arc::new(Storage::new());
    let peer_url = start_server_with_key(peer_storage.clone(), "//Bob").await;

    // Push from writer to peer and collect receipt.
    let writer_sync = ReplicaSync::new(writer_storage.clone());
    let results = writer_sync
        .push_to_peers(BUCKET, committed_root, std::slice::from_ref(&peer_url))
        .await;

    assert_eq!(results.len(), 1);
    let (_, receipt_result) = &results[0];
    let receipt = receipt_result.as_ref().expect("push must succeed");

    // Peer must actually hold the data now.
    assert_eq!(
        peer_storage.total_nodes(),
        writer_storage.total_nodes(),
        "peer must hold all pushed nodes"
    );

    // Valid receipt verifies against the peer's public key.
    assert!(
        writer_sync.verify_receipt(BUCKET, committed_root, receipt),
        "valid receipt must verify"
    );
}

#[tokio::test]
async fn test_l3_tampered_receipt_fails_verification() {
    let writer_storage = Arc::new(Storage::new());
    seed(&writer_storage, 2);
    let committed_root = writer_storage.get_bucket(BUCKET).unwrap().mmr_root;

    let peer_storage = Arc::new(Storage::new());
    let peer_url = start_server_with_key(peer_storage.clone(), "//Bob").await;

    let writer_sync = ReplicaSync::new(writer_storage.clone());
    let results = writer_sync
        .push_to_peers(BUCKET, committed_root, &[peer_url])
        .await;

    let (_, receipt_result) = &results[0];
    let receipt = receipt_result.as_ref().unwrap();

    // A different root means the signed payload doesn't match → reject.
    let wrong_root = sp_core::H256::repeat_byte(0xDE);
    assert!(
        !writer_sync.verify_receipt(BUCKET, wrong_root, receipt),
        "receipt over wrong root must not verify"
    );
}
