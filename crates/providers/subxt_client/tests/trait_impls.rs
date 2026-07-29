// SPDX-License-Identifier: Apache-2.0

//! Network-free tests for the chain-client trait impls.
//!
//! These drive the coordinator traits through the real [`SubxtChainClient`]
//! against a chain watch channel that never connected, so every call must
//! come back with the deterministic "connection not established" error.
//!
//! The two `submit_*` trait methods are not exercised here: they need proof
//! fixtures and ride the submit retry loop rather than failing fast.

use provider_subxt_client::chain_connection::ChainHandle;
use provider_subxt_client::{ChallengeChainClient, ReplicaSyncChainClient, SubxtChainClient};

const NOT_CONNECTED: &str = "Chain connection not established yet";

fn unconnected_client() -> SubxtChainClient {
    let (_tx, rx) = tokio::sync::watch::channel::<Option<ChainHandle>>(None);
    SubxtChainClient::new(rx, "//Alice").expect("valid dev seed")
}

#[tokio::test]
async fn replica_sync_trait_reports_missing_connection() {
    let client = unconnected_client();

    let err = ReplicaSyncChainClient::get_current_block(&client)
        .await
        .expect_err("no chain connection");
    assert!(err.to_string().contains(NOT_CONNECTED), "got: {err}");

    let err = ReplicaSyncChainClient::fetch_replica_agreements(&client, "0xdeadbeef", vec![])
        .await
        .expect_err("no chain connection");
    assert!(err.to_string().contains(NOT_CONNECTED), "got: {err}");

    let err = ReplicaSyncChainClient::fetch_bucket_snapshot(&client, 1)
        .await
        .expect_err("no chain connection");
    assert!(err.to_string().contains(NOT_CONNECTED), "got: {err}");

    let err = ReplicaSyncChainClient::fetch_primary_endpoints(&client, 1)
        .await
        .expect_err("no chain connection");
    assert!(err.to_string().contains(NOT_CONNECTED), "got: {err}");
}

#[tokio::test]
async fn challenge_trait_reports_missing_connection() {
    let client = unconnected_client();

    let err = ChallengeChainClient::poll_challenges(&client)
        .await
        .expect_err("no chain connection");
    assert!(err.to_string().contains(NOT_CONNECTED), "got: {err}");

    let err = ChallengeChainClient::fetch_challenge(&client, 100, 0)
        .await
        .expect_err("no chain connection");
    assert!(err.to_string().contains(NOT_CONNECTED), "got: {err}");
}
