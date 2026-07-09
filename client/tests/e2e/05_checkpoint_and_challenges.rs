// SPDX-License-Identifier: Apache-2.0

//! E2E Workflow 05 — Checkpoint and Challenges (happy-case port of
//! `examples/papi/e2e/05-checkpoint-and-challenges.ts`).
//!
//! Accounts: `alice` (provider), `bob` (client, admin, challenger).
//!
//! Covers client checkpoint, off-chain challenge + defense, on-chain
//! challenge + defense, provider-initiated checkpoint + reward, and claiming
//! checkpoint rewards - using the G3-G9 gap functions added in Stage 2.
//! Failure cases from the JS reference (5.6-5.7) are out of scope for this
//! happy-case suite.
//!
//! Requires a running parachain (`just start-chain`) and a live provider
//! node registered on-chain (`just start-provider`); skipped (not failed)
//! when either is unreachable.

#[path = "common.rs"]
mod e2e_common;

use e2e_common::common::{chain_guard, dev_account, dev_ss58, CHAIN_WS};
use e2e_common::{
    admin_for, current_block, ensure_provider_registered, negotiate_and_establish, wait_for_block,
    PROVIDER_URL,
};
use sp_core::sr25519;
use sp_runtime::MultiSignature;
use storage_client::substrate::parse_h256;
use storage_client::{
    ChallengerClient, ChunkLocation, ChunkingStrategy, ClientConfig, Commitment, ProviderClient,
    StorageUserClient,
};

const WINDOW_INTERVAL: u32 = 20;
const WINDOW_GRACE: u32 = 10;
const HEADROOM: u32 = 8;
const POOL_AMOUNT: u128 = 5_000_000_000_000;

/// `bob` is the bucket admin (he redeemed the terms), so he has Writer
/// access - required by the provider node's default auth-enabled config for
/// `upload`/`commit`.
fn user_client() -> StorageUserClient {
    StorageUserClient::new(ClientConfig {
        chain_ws_url: CHAIN_WS.to_string(),
        provider_urls: vec![PROVIDER_URL.to_string()],
        timeout_secs: 30,
        enable_retries: false,
    })
    .expect("ClientConfig should be valid")
    .with_dev_signer("bob")
    .expect("bob is a valid dev signer")
}

async fn challenger_for(name: &str) -> Option<ChallengerClient> {
    let mut challenger = ChallengerClient::new(
        ClientConfig {
            chain_ws_url: CHAIN_WS.to_string(),
            provider_urls: vec![],
            timeout_secs: 30,
            enable_retries: false,
        },
        dev_ss58(name),
    )
    .ok()?;
    if challenger.connect().await.is_err() {
        return None;
    }
    challenger.set_dev_signer(name).ok()?;
    Some(challenger)
}

async fn provider_client_for(name: &str) -> Option<ProviderClient> {
    let mut provider = ProviderClient::new(
        ClientConfig {
            chain_ws_url: CHAIN_WS.to_string(),
            provider_urls: vec![],
            timeout_secs: 30,
            enable_retries: false,
        },
        dev_ss58(name),
    )
    .ok()?;
    if provider.connect().await.is_err() {
        return None;
    }
    provider.set_dev_signer(name).ok()?;
    Some(provider)
}

/// Decode a provider node's `0x`-hex signature into raw bytes (for
/// `submit_checkpoint`/`challenge_offchain`, which take `Vec<u8>` directly).
fn decode_hex_bytes(hex_sig: &str) -> Vec<u8> {
    hex::decode(hex_sig.trim_start_matches("0x")).expect("valid hex signature")
}

/// Decode a provider node's `0x`-hex sr25519 signature into a `MultiSignature`
/// (for `provider_checkpoint`, which takes a typed signature).
fn decode_signature(hex_sig: &str) -> MultiSignature {
    let bytes = decode_hex_bytes(hex_sig);
    let mut raw = [0u8; 64];
    raw.copy_from_slice(&bytes);
    MultiSignature::Sr25519(sr25519::Signature::from_raw(raw))
}

#[tokio::test]
async fn checkpoint_and_challenges_lifecycle() {
    let _guard = chain_guard().await;

    let Some(_provider) = ensure_provider_registered("alice", 1).await else {
        eprintln!("skipping: chain or provider node unreachable");
        return;
    };
    let alice_ss58 = dev_ss58("alice");
    let alice_account = dev_account("alice");

    let Some(bucket_id) = negotiate_and_establish("bob", &alice_ss58, 1_048_576, 200, 1).await
    else {
        eprintln!("skipping: chain unreachable");
        return;
    };

    // Upload + commit a chunk so the bucket has data to checkpoint/challenge.
    let user = user_client();
    let payload = b"checkpoint-test payload";
    let Some(upload_nonce) = current_block().await else {
        eprintln!("skipping: chain unreachable");
        return;
    };
    let data_root = user
        .upload(bucket_id, payload, ChunkingStrategy::default())
        .await
        .expect("upload should succeed");
    let commit = user
        .commit(bucket_id, vec![data_root], upload_nonce as u64)
        .await
        .expect("commit should succeed");

    let Some(bob) = admin_for("bob").await else {
        eprintln!("skipping: chain unreachable");
        return;
    };

    // 5.1 - Client checkpoint.
    let Some(ck_nonce) = current_block().await else {
        eprintln!("skipping: chain unreachable");
        return;
    };
    let ck = user
        .get_checkpoint_signature(bucket_id, ck_nonce as u64)
        .await
        .expect("get_checkpoint_signature should succeed");
    let ck_commitment = Commitment {
        mmr_root: parse_h256(&ck.mmr_root).expect("valid mmr_root hex"),
        start_seq: ck.start_seq,
        leaf_count: ck.leaf_count,
    };
    bob.submit_checkpoint(
        bucket_id,
        ck_commitment,
        ck_nonce as u64,
        vec![(alice_ss58.clone(), decode_hex_bytes(&ck.provider_signature))],
    )
    .await
    .expect("submit_checkpoint should succeed");

    // 5.2 - Off-chain challenge + defense, using the commit's own signature
    // (signed over the real leaf_count, per the provider node's /commit handler).
    let Some(challenger) = challenger_for("bob").await else {
        eprintln!("skipping: chain unreachable");
        return;
    };
    let Some(alice_provider) = provider_client_for("alice").await else {
        eprintln!("skipping: chain unreachable");
        return;
    };
    let commit_commitment = Commitment {
        mmr_root: parse_h256(&commit.mmr_root).expect("valid mmr_root hex"),
        start_seq: commit.start_seq,
        leaf_count: commit.leaf_count,
    };
    let target = ChunkLocation {
        leaf_index: commit.leaf_indices[0],
        chunk_index: 0,
    };
    let challenge_id = challenger
        .challenge_offchain(
            bucket_id,
            alice_ss58.clone(),
            commit_commitment,
            target,
            commit.nonce,
            decode_hex_bytes(&commit.provider_signature),
        )
        .await
        .expect("challenge_offchain should succeed");
    let (chunk_data, mmr_proof, chunk_proof) =
        ProviderClient::fetch_challenge_proof(CHAIN_WS, PROVIDER_URL, challenge_id)
            .await
            .expect("fetch_challenge_proof should succeed");
    alice_provider
        .respond_to_challenge(
            (challenge_id.deadline, challenge_id.index),
            chunk_data,
            &mmr_proof,
            &chunk_proof,
        )
        .await
        .expect("respond_to_challenge (offchain) should succeed");

    // 5.3 - On-chain challenge + defense, against the checkpoint submitted in 5.1.
    let challenge_id = challenger
        .challenge_checkpoint(bucket_id, alice_ss58.clone(), target)
        .await
        .expect("challenge_checkpoint should succeed");
    let (chunk_data, mmr_proof, chunk_proof) =
        ProviderClient::fetch_challenge_proof(CHAIN_WS, PROVIDER_URL, challenge_id)
            .await
            .expect("fetch_challenge_proof should succeed");
    alice_provider
        .respond_to_challenge(
            (challenge_id.deadline, challenge_id.index),
            chunk_data,
            &mmr_proof,
            &chunk_proof,
        )
        .await
        .expect("respond_to_challenge (on-chain) should succeed");

    // 5.4 - Provider-initiated checkpoint + reward.
    bob.configure_checkpoint_window(bucket_id, WINDOW_INTERVAL, WINDOW_GRACE, true)
        .await
        .expect("configure_checkpoint_window should succeed");
    bob.fund_checkpoint_pool(bucket_id, POOL_AMOUNT)
        .await
        .expect("fund_checkpoint_pool should succeed");

    let Some(mut block) = current_block().await else {
        eprintln!("skipping: chain unreachable");
        return;
    };
    let mut window_num = block / WINDOW_INTERVAL;
    let next_window_start = (window_num + 1) * WINDOW_INTERVAL;
    if next_window_start - block < HEADROOM {
        let Some(()) = wait_for_block(next_window_start - 1).await else {
            eprintln!("skipping: chain unreachable while waiting for window");
            return;
        };
        let Some(refreshed) = current_block().await else {
            eprintln!("skipping: chain unreachable");
            return;
        };
        block = refreshed;
        window_num = block / WINDOW_INTERVAL;
    }
    let window = window_num as u64;

    let duty = ProviderClient::fetch_checkpoint_duty(PROVIDER_URL, bucket_id)
        .await
        .expect("fetch_checkpoint_duty should succeed");
    assert!(duty.ready, "provider should be ready to checkpoint");
    let signed = ProviderClient::sign_checkpoint_proposal(PROVIDER_URL, bucket_id, &duty, window)
        .await
        .expect("sign_checkpoint_proposal should succeed");
    assert!(
        signed.agreed,
        "provider should agree to sign its own proposal"
    );

    let reward = alice_provider
        .provider_checkpoint(
            bucket_id,
            Commitment {
                mmr_root: parse_h256(&duty.mmr_root).expect("valid mmr_root hex"),
                start_seq: duty.start_seq,
                leaf_count: duty.leaf_count,
            },
            window,
            vec![(alice_account.clone(), decode_signature(&signed.signature))],
        )
        .await
        .expect("provider_checkpoint should succeed");
    assert!(reward > 0, "reward should be positive");

    // 5.5 - Claim checkpoint rewards.
    let claimed = alice_provider
        .claim_checkpoint_rewards(bucket_id)
        .await
        .expect("claim_checkpoint_rewards should succeed");
    assert!(claimed > 0, "claimed amount should be positive");
}
