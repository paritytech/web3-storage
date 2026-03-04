//! Layer 0 - Storage Integration Tests
//!
//! Port of the PAPI JS test (examples/papi/full-flow.js) to Rust.
//!
//! Flow:
//! 0. Spawn network + provider
//! 1. Register provider (Alice), create bucket (Bob), request + accept agreement
//! 2. Upload data, commit to MMR, verify download
//! 3. Off-chain challenge
//! 4. Respond to off-chain challenge → ChallengeDefended
//! 5. Submit checkpoint
//! 6. Checkpoint challenge
//! 7. Respond to checkpoint challenge → ChallengeDefended

use crate::common::{
    config::*,
    setup::{client_config, hex_to_bytes, TestEnvironment},
};
use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use sp_core::H256;
use storage_client::{AdminClient, ChallengerClient, StorageClient};
use storage_primitives::{MerkleProof, MmrLeaf, MmrProof};

// ─────────────────────────────────────────────────────────────────────────────
// HTTP response types (matching provider-node API)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct MmrProofResponse {
    leaf: MmrLeafData,
    proof: MmrProofData,
}

#[derive(Debug, Deserialize)]
struct MmrLeafData {
    data_root: String,
    data_size: u64,
    total_size: u64,
}

#[derive(Debug, Deserialize)]
struct MmrProofData {
    peaks: Vec<String>,
    siblings: Vec<String>,
    path: Vec<bool>,
}

#[derive(Debug, Deserialize)]
struct ChunkProofResponse {
    /// Present in the provider response but not needed for challenge responses.
    #[allow(dead_code)]
    chunk_hash: String,
    chunk_data: Option<String>,
    proof: MerkleProofDataResp,
}

#[derive(Debug, Deserialize)]
struct MerkleProofDataResp {
    siblings: Vec<String>,
    path: Vec<bool>,
}

#[derive(Debug, Deserialize)]
struct CheckpointSignatureResp {
    mmr_root: String,
    start_seq: u64,
    leaf_count: u64,
    provider_signature: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// Hex utilities
// ─────────────────────────────────────────────────────────────────────────────

fn hex_to_h256(hex: &str) -> Result<H256> {
    let bytes = hex_to_bytes(hex)?;
    if bytes.len() != 32 {
        anyhow::bail!("Expected 32 bytes for H256, got {}", bytes.len());
    }
    Ok(H256::from_slice(&bytes))
}

// ─────────────────────────────────────────────────────────────────────────────
// Provider HTTP helpers
// ─────────────────────────────────────────────────────────────────────────────

async fn fetch_mmr_proof(
    http: &reqwest::Client,
    base_url: &str,
    bucket_id: u64,
    leaf_index: u64,
) -> Result<MmrProofResponse> {
    let resp = http
        .get(format!("{}/mmr_proof", base_url))
        .query(&[
            ("bucket_id", bucket_id.to_string()),
            ("leaf_index", leaf_index.to_string()),
        ])
        .send()
        .await
        .context("mmr_proof request failed")?;
    if !resp.status().is_success() {
        anyhow::bail!("mmr_proof failed: {}", resp.text().await?);
    }
    resp.json().await.context("mmr_proof response parse failed")
}

async fn fetch_chunk_proof(
    http: &reqwest::Client,
    base_url: &str,
    data_root: &str,
    chunk_index: u64,
) -> Result<ChunkProofResponse> {
    let resp = http
        .get(format!("{}/chunk_proof", base_url))
        .query(&[
            ("data_root", data_root.to_string()),
            ("chunk_index", chunk_index.to_string()),
        ])
        .send()
        .await
        .context("chunk_proof request failed")?;
    if !resp.status().is_success() {
        anyhow::bail!("chunk_proof failed: {}", resp.text().await?);
    }
    resp.json()
        .await
        .context("chunk_proof response parse failed")
}

async fn fetch_checkpoint_signature(
    http: &reqwest::Client,
    base_url: &str,
    bucket_id: u64,
) -> Result<CheckpointSignatureResp> {
    let resp = http
        .get(format!("{}/checkpoint-signature", base_url))
        .query(&[("bucket_id", bucket_id.to_string())])
        .send()
        .await
        .context("checkpoint-signature request failed")?;
    if !resp.status().is_success() {
        anyhow::bail!("checkpoint-signature failed: {}", resp.text().await?);
    }
    resp.json()
        .await
        .context("checkpoint-signature response parse failed")
}

// ─────────────────────────────────────────────────────────────────────────────
// Challenge response helper
// ─────────────────────────────────────────────────────────────────────────────

/// Fetch proofs from the provider and respond to a challenge on-chain.
async fn respond_to_challenge(
    provider_client: &storage_client::ProviderClient,
    http: &reqwest::Client,
    base_url: &str,
    challenge_id: storage_client::challenger::ChallengeId,
    bucket_id: u64,
    leaf_index: u64,
    chunk_index: u64,
) -> Result<()> {
    // Fetch MMR proof (proves leaf is in the MMR tree)
    let mmr_resp = fetch_mmr_proof(http, base_url, bucket_id, leaf_index).await?;

    // Fetch chunk proof (proves chunk is in the data blob)
    let chunk_resp =
        fetch_chunk_proof(http, base_url, &mmr_resp.leaf.data_root, chunk_index).await?;

    // Decode the base64-encoded chunk data
    use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
    let chunk_data = BASE64
        .decode(
            chunk_resp
                .chunk_data
                .as_ref()
                .ok_or_else(|| anyhow!("chunk_proof response missing chunk_data"))?,
        )
        .context("Failed to decode chunk_data base64")?;

    // Convert JSON proof structures to on-chain types
    let mmr_proof = MmrProof {
        peaks: mmr_resp
            .proof
            .peaks
            .iter()
            .map(|h| hex_to_h256(h))
            .collect::<Result<Vec<_>>>()?,
        leaf: MmrLeaf {
            data_root: hex_to_h256(&mmr_resp.leaf.data_root)?,
            data_size: mmr_resp.leaf.data_size,
            total_size: mmr_resp.leaf.total_size,
        },
        leaf_proof: MerkleProof {
            siblings: mmr_resp
                .proof
                .siblings
                .iter()
                .map(|h| hex_to_h256(h))
                .collect::<Result<Vec<_>>>()?,
            path: mmr_resp.proof.path,
        },
    };

    let chunk_proof = MerkleProof {
        siblings: chunk_resp
            .proof
            .siblings
            .iter()
            .map(|h| hex_to_h256(h))
            .collect::<Result<Vec<_>>>()?,
        path: chunk_resp.proof.path,
    };

    // Submit the response on-chain
    provider_client
        .respond_to_challenge(
            (challenge_id.deadline, challenge_id.index),
            chunk_data,
            &mmr_proof,
            &chunk_proof,
        )
        .await
        .context("respond_to_challenge failed")?;

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Main test
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn storage_full_flow_test() -> Result<()> {
    log::info!("=== Layer 0: Storage Full Flow Test ===");

    let env = TestEnvironment::spawn().await?;
    let config = client_config(&env.chain_ws, &env.provider_url);
    let alice_provider = env.alice_provider;

    // ═══════════════════════════════════════════════════════════════════
    // Step 1: Setup - create bucket, create agreement
    // ═══════════════════════════════════════════════════════════════════
    log::info!("\n=== Step 1: Setup ===");

    // Bob = bucket admin / challenger
    let mut bob_admin = AdminClient::new(config.clone(), BOB_SS58.to_string())
        .context("Failed to create AdminClient")?;
    bob_admin
        .connect()
        .await
        .context("AdminClient connect failed")?;
    bob_admin
        .set_dev_signer("bob")
        .context("AdminClient set_dev_signer failed")?;

    let mut bob_challenger = ChallengerClient::new(config.clone(), BOB_SS58.to_string())
        .context("Failed to create ChallengerClient")?;
    bob_challenger
        .connect()
        .await
        .context("ChallengerClient connect failed")?;
    bob_challenger
        .set_dev_signer("bob")
        .context("ChallengerClient set_dev_signer failed")?;

    // Create bucket (Bob)
    log::info!("  Creating bucket...");
    bob_admin
        .create_bucket(1)
        .await
        .context("create_bucket failed")?;
    log::info!("  Bucket created (ID: {})", BUCKET_ID);

    // Request agreement (Bob)
    log::info!("  Requesting agreement (Bob)...");
    bob_admin
        .request_agreement(
            BUCKET_ID,
            ALICE_SS58.to_string(),
            AGREEMENT_MAX_BYTES,
            AGREEMENT_DURATION_BLOCKS,
            AGREEMENT_MAX_PAYMENT,
            None,
        )
        .await
        .context("request_agreement failed")?;
    log::info!("  Agreement requested");

    // Accept agreement (Alice)
    log::info!("  Accepting agreement (Alice)...");
    alice_provider
        .accept_agreement(BUCKET_ID)
        .await
        .context("accept_agreement failed")?;
    log::info!("  Agreement accepted");

    // ═══════════════════════════════════════════════════════════════════
    // Step 2: Upload data
    // ═══════════════════════════════════════════════════════════════════
    log::info!("\n=== Step 2: Upload data ===");

    let storage_client = StorageClient::new(&env.provider_url);
    let data = format!(
        "Hello, Web3 Storage! [zombienet-sdk test at {}]",
        unix_timestamp()
    );
    let data_bytes = data.as_bytes();

    log::info!("  Uploading data ({} bytes)...", data_bytes.len());
    let data_root = storage_client
        .upload(BUCKET_ID, data_bytes, Default::default())
        .await
        .context("upload failed")?;
    log::info!("  Data root: {:?}", data_root);

    log::info!("  Committing to MMR...");
    let commit_resp = storage_client
        .commit(BUCKET_ID, vec![data_root])
        .await
        .context("commit failed")?;
    log::info!("  MMR root: {}", commit_resp.mmr_root);
    log::info!("  Leaf indices: {:?}", commit_resp.leaf_indices);

    log::info!("  Verifying download...");
    let downloaded = storage_client
        .read(&data_root, 0, data_bytes.len() as u64)
        .await
        .context("read failed")?;
    assert_eq!(
        downloaded, data_bytes,
        "Downloaded data does not match uploaded data"
    );
    log::info!(
        "  Upload verified: data matches ({} bytes)",
        data_bytes.len()
    );

    let leaf_index = *commit_resp
        .leaf_indices
        .first()
        .ok_or_else(|| anyhow!("No leaf indices returned from commit"))?;

    // ═══════════════════════════════════════════════════════════════════
    // Step 3: Off-chain challenge
    // ═══════════════════════════════════════════════════════════════════
    log::info!("\n=== Step 3: Off-chain challenge ===");

    let mmr_root = hex_to_h256(&commit_resp.mmr_root)?;
    let provider_sig_bytes = hex_to_bytes(&commit_resp.provider_signature)?;

    let challenge_id_1 = bob_challenger
        .challenge_offchain(
            BUCKET_ID,
            ALICE_SS58.to_string(),
            mmr_root,
            commit_resp.start_seq,
            leaf_index,
            0, // chunk_index
            provider_sig_bytes,
        )
        .await
        .context("challenge_offchain failed")?;
    log::info!(
        "  Challenge created: deadline={}, index={}",
        challenge_id_1.deadline,
        challenge_id_1.index
    );

    // ═══════════════════════════════════════════════════════════════════
    // Step 4: Respond to off-chain challenge
    // ═══════════════════════════════════════════════════════════════════
    log::info!("\n=== Step 4: Respond to off-chain challenge ===");

    let http = reqwest::Client::new();
    respond_to_challenge(
        &alice_provider,
        &http,
        &env.provider_url,
        challenge_id_1,
        BUCKET_ID,
        leaf_index,
        0,
    )
    .await?;
    log::info!("  Challenge defended (1/2)");

    // ═══════════════════════════════════════════════════════════════════
    // Step 5: Submit checkpoint
    // ═══════════════════════════════════════════════════════════════════
    log::info!("\n=== Step 5: Submit checkpoint ===");

    let checkpoint_sig = fetch_checkpoint_signature(&http, &env.provider_url, BUCKET_ID).await?;
    log::info!("  Checkpoint MMR root: {}", checkpoint_sig.mmr_root);
    log::info!("  Checkpoint leaf_count: {}", checkpoint_sig.leaf_count);

    let checkpoint_mmr_root = hex_to_h256(&checkpoint_sig.mmr_root)?;
    let checkpoint_provider_sig = hex_to_bytes(&checkpoint_sig.provider_signature)?;

    bob_admin
        .submit_checkpoint(
            BUCKET_ID,
            checkpoint_mmr_root,
            checkpoint_sig.start_seq,
            checkpoint_sig.leaf_count,
            vec![(ALICE_SS58.to_string(), checkpoint_provider_sig)],
        )
        .await
        .context("submit_checkpoint failed")?;
    log::info!("  Checkpoint submitted");

    // ═══════════════════════════════════════════════════════════════════
    // Step 6: Checkpoint challenge
    // ═══════════════════════════════════════════════════════════════════
    log::info!("\n=== Step 6: Checkpoint challenge ===");

    let challenge_id_2 = bob_challenger
        .challenge_checkpoint(
            BUCKET_ID,
            ALICE_SS58.to_string(),
            leaf_index,
            0, // chunk_index
        )
        .await
        .context("challenge_checkpoint failed")?;
    log::info!(
        "  Challenge created: deadline={}, index={}",
        challenge_id_2.deadline,
        challenge_id_2.index
    );

    // ═══════════════════════════════════════════════════════════════════
    // Step 7: Respond to checkpoint challenge
    // ═══════════════════════════════════════════════════════════════════
    log::info!("\n=== Step 7: Respond to checkpoint challenge ===");

    respond_to_challenge(
        &alice_provider,
        &http,
        &env.provider_url,
        challenge_id_2,
        BUCKET_ID,
        leaf_index,
        0,
    )
    .await?;
    log::info!("  Challenge defended (2/2)");

    // ═══════════════════════════════════════════════════════════════════
    // Summary
    // ═══════════════════════════════════════════════════════════════════
    log::info!("\n=== PASSED: Both challenges were defended! ===");
    log::info!("  - Registered provider");
    log::info!("  - Created bucket + agreement");
    log::info!("  - Uploaded + verified data");
    log::info!("  - Off-chain challenge defended");
    log::info!("  - Checkpoint submitted + challenge defended");

    Ok(())
}

fn unix_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}.{}", duration.as_secs(), duration.subsec_millis())
}
