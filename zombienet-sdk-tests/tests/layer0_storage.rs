//! Layer 0 - Storage Integration Tests
//!
//! Port of the PAPI JS test (examples/papi/full-flow.js) to Rust.
//!
//! Flow:
//! 1. Spawn network + provider
//! 2. Register provider (Alice)
//! 3. Create bucket (Bob)
//! 4. Request + accept agreement
//! 5. Upload data, commit to MMR, verify download
//! 6. Off-chain challenge → respond → ChallengeDefended
//! 7. Submit checkpoint
//! 8. Checkpoint challenge → respond → ChallengeDefended
//! 9. Assert both challenges were defended

use crate::common::{
    config::provider_url,
    network::{build_network_config, spawn_network, wait_for_collator_ws},
    provider::ProviderProcess,
};
use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use sp_core::H256;
use storage_client::{AdminClient, ChallengerClient, ClientConfig, ProviderClient, StorageClient};
use storage_primitives::{MerkleProof, MmrLeaf, MmrProof};

// Well-known dev account addresses (SS58)
const ALICE_SS58: &str = "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY";
const BOB_SS58: &str = "5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty";

// Alice's sr25519 public key (well-known dev account)
const ALICE_PUBLIC_KEY_HEX: &str =
    "d43593c715fdd31c61141abd04a99fd6822c8558854ccde39a5684e7a56da27d";

// First bucket ID is 0 (auto-incremented from 0 on a fresh chain)
const BUCKET_ID: u64 = 0;

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

fn hex_to_bytes(hex: &str) -> Result<Vec<u8>> {
    let hex = hex.strip_prefix("0x").unwrap_or(hex);
    if hex.len() % 2 != 0 {
        anyhow::bail!("Invalid hex length: {}", hex.len());
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&hex[i..i + 2], 16)
                .map_err(|e| anyhow!("Invalid hex at offset {}: {}", i, e))
        })
        .collect()
}

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
        .await?;
    if !resp.status().is_success() {
        anyhow::bail!("mmr_proof failed: {}", resp.text().await?);
    }
    Ok(resp.json().await?)
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
        .await?;
    if !resp.status().is_success() {
        anyhow::bail!("chunk_proof failed: {}", resp.text().await?);
    }
    Ok(resp.json().await?)
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
        .await?;
    if !resp.status().is_success() {
        anyhow::bail!("checkpoint-signature failed: {}", resp.text().await?);
    }
    Ok(resp.json().await?)
}

// ─────────────────────────────────────────────────────────────────────────────
// Challenge response helper
// ─────────────────────────────────────────────────────────────────────────────

/// Fetch proofs from the provider and respond to a challenge on-chain.
async fn respond_to_challenge(
    provider_client: &ProviderClient,
    http: &reqwest::Client,
    base_url: &str,
    challenge_id: storage_client::challenger::ChallengeId,
    bucket_id: u64,
    leaf_index: u64,
    chunk_index: u64,
) -> Result<()> {
    // 1. Fetch MMR proof from provider
    let mmr_resp = fetch_mmr_proof(http, base_url, bucket_id, leaf_index).await?;

    // 2. Fetch chunk proof from provider
    let chunk_resp =
        fetch_chunk_proof(http, base_url, &mmr_resp.leaf.data_root, chunk_index).await?;

    // 3. Decode chunk data
    use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
    let chunk_data = BASE64
        .decode(
            chunk_resp
                .chunk_data
                .as_ref()
                .ok_or_else(|| anyhow!("chunk_proof response missing chunk_data"))?,
        )
        .context("Failed to decode chunk_data base64")?;

    // 4. Convert to storage_primitives types
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

    // 5. Respond on-chain
    provider_client
        .respond_to_challenge(
            (challenge_id.deadline, challenge_id.index),
            chunk_data,
            &mmr_proof,
            &chunk_proof,
        )
        .await
        .map_err(|e| anyhow!("respond_to_challenge failed: {}", e))?;

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Main test
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn storage_full_flow_test() -> Result<()> {
    let _ = env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("off,layer0_storage=info,zombienet_sdk=info"),
    )
    .try_init();

    log::info!("=== Layer 0: Storage Full Flow Test ===");

    // Step 0: Spawn network + provider
    log::info!("Step 0: Spawning network and provider...");
    let config = build_network_config()?;
    let network = spawn_network(config).await?;
    let chain_ws = wait_for_collator_ws(&network, "collator-alice").await?;
    let _provider = ProviderProcess::spawn(&chain_ws).await?;
    let prov_url = provider_url();

    log::info!("  Chain: {}", chain_ws);
    log::info!("  Provider: {}", prov_url);

    // Create client config pointing to the spawned network
    let client_config = ClientConfig {
        chain_ws_url: chain_ws.clone(),
        provider_urls: vec![prov_url.clone()],
        timeout_secs: 60,
        enable_retries: true,
    };

    // ═══════════════════════════════════════════════════════════════════
    // Step 1: Setup - register provider, create bucket, create agreement
    // ═══════════════════════════════════════════════════════════════════
    log::info!("\n=== Step 1: Setup ===");

    // Alice = provider
    let mut alice_provider = ProviderClient::new(client_config.clone(), ALICE_SS58.to_string())
        .map_err(|e| anyhow!("Failed to create ProviderClient: {}", e))?;
    alice_provider
        .connect()
        .await
        .map_err(|e| anyhow!("ProviderClient connect failed: {}", e))?;
    alice_provider
        .set_dev_signer("alice")
        .map_err(|e| anyhow!("ProviderClient set_dev_signer failed: {}", e))?;

    // Bob = bucket admin / challenger
    let mut bob_admin = AdminClient::new(client_config.clone(), BOB_SS58.to_string())
        .map_err(|e| anyhow!("Failed to create AdminClient: {}", e))?;
    bob_admin
        .connect()
        .await
        .map_err(|e| anyhow!("AdminClient connect failed: {}", e))?;
    bob_admin
        .set_dev_signer("bob")
        .map_err(|e| anyhow!("AdminClient set_dev_signer failed: {}", e))?;

    let mut bob_challenger = ChallengerClient::new(client_config.clone(), BOB_SS58.to_string())
        .map_err(|e| anyhow!("Failed to create ChallengerClient: {}", e))?;
    bob_challenger
        .connect()
        .await
        .map_err(|e| anyhow!("ChallengerClient connect failed: {}", e))?;
    bob_challenger
        .set_dev_signer("bob")
        .map_err(|e| anyhow!("ChallengerClient set_dev_signer failed: {}", e))?;

    // Register provider (Alice)
    log::info!("  Registering provider (Alice)...");
    let multiaddr = "/ip4/127.0.0.1/tcp/3333".to_string();
    let alice_public_key = hex_to_bytes(ALICE_PUBLIC_KEY_HEX)?;
    alice_provider
        .register(
            multiaddr,
            alice_public_key,
            1_000_000_000_000_000, // 1000 tokens
        )
        .await
        .map_err(|e| anyhow!("register_provider failed: {}", e))?;
    log::info!("  Provider registered");

    // Create bucket (Bob)
    log::info!("  Creating bucket...");
    bob_admin
        .create_bucket(1)
        .await
        .map_err(|e| anyhow!("create_bucket failed: {}", e))?;
    log::info!("  Bucket created (ID: {})", BUCKET_ID);

    // Request agreement (Bob)
    log::info!("  Requesting agreement (Bob)...");
    bob_admin
        .request_agreement(
            BUCKET_ID,
            ALICE_SS58.to_string(),
            1_073_741_824,   // 1 GB max_bytes
            100_000,         // duration in blocks
            100_000_000_000, // max payment
            None,            // no replica params
        )
        .await
        .map_err(|e| anyhow!("request_agreement failed: {}", e))?;
    log::info!("  Agreement requested");

    // Accept agreement (Alice)
    log::info!("  Accepting agreement (Alice)...");
    alice_provider
        .accept_agreement(BUCKET_ID)
        .await
        .map_err(|e| anyhow!("accept_agreement failed: {}", e))?;
    log::info!("  Agreement accepted");

    // ═══════════════════════════════════════════════════════════════════
    // Step 2: Upload data
    // ═══════════════════════════════════════════════════════════════════
    log::info!("\n=== Step 2: Upload data ===");

    let storage_client = StorageClient::new(&prov_url);
    let data = format!(
        "Hello, Web3 Storage! [zombienet-sdk test at {}]",
        chrono_timestamp()
    );
    let data_bytes = data.as_bytes();

    // Upload data using the StorageClient
    log::info!("  Uploading data ({} bytes)...", data_bytes.len());
    let data_root = storage_client
        .upload(BUCKET_ID, data_bytes, Default::default())
        .await
        .map_err(|e| anyhow!("upload failed: {}", e))?;
    log::info!("  Data root: {:?}", data_root);

    // Commit to MMR
    log::info!("  Committing to MMR...");
    let commit_resp = storage_client
        .commit(BUCKET_ID, vec![data_root])
        .await
        .map_err(|e| anyhow!("commit failed: {}", e))?;
    log::info!("  MMR root: {}", commit_resp.mmr_root);
    log::info!("  Leaf indices: {:?}", commit_resp.leaf_indices);

    // Verify download
    log::info!("  Verifying download...");
    let downloaded = storage_client
        .read(&data_root, 0, data_bytes.len() as u64)
        .await
        .map_err(|e| anyhow!("read failed: {}", e))?;
    assert_eq!(
        downloaded, data_bytes,
        "Downloaded data does not match uploaded data"
    );
    log::info!(
        "  Upload verified: data matches ({} bytes)",
        data_bytes.len()
    );

    let leaf_index = commit_resp.leaf_indices[0];

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
        .map_err(|e| anyhow!("challenge_offchain failed: {}", e))?;
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
        &prov_url,
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

    let checkpoint_sig = fetch_checkpoint_signature(&http, &prov_url, BUCKET_ID).await?;
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
        .map_err(|e| anyhow!("submit_checkpoint failed: {}", e))?;
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
        .map_err(|e| anyhow!("challenge_checkpoint failed: {}", e))?;
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
        &prov_url,
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

/// Simple timestamp for test data uniqueness.
fn chrono_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let duration = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    format!("{}.{}", duration.as_secs(), duration.subsec_millis())
}
