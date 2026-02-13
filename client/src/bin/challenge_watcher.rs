//! Challenge Watcher - Auto-responds to challenges on behalf of a provider.
//!
//! This binary watches the chain for ChallengeCreated events targeting the
//! configured provider, fetches proofs from the provider's HTTP API, and
//! submits respond_to_challenge extrinsics.
//!
//! Environment variables:
//!   CHAIN_WS    - WebSocket URL for the chain (default: ws://127.0.0.1:9944)
//!   PROVIDER_URL - Provider HTTP URL (default: http://127.0.0.1:3000)
//!   SEED        - Signing seed/derivation path (default: //Alice)
//!
//! Usage:
//!   cargo run --release -p storage-client --bin challenge_watcher
//!   SEED=//Alice CHAIN_WS=ws://127.0.0.1:9944 cargo run --release -p storage-client --bin challenge_watcher

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use reqwest::Client as HttpClient;
use scale_value::At;
use serde::Deserialize;
use sp_core::{sr25519, Pair, H256};
use storage_client::substrate::{extrinsics, storage, SubstrateClient};
use storage_primitives::{MerkleProof, MmrLeaf, MmrProof};

fn hex_decode(s: &str) -> Result<Vec<u8>, String> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    hex::decode(s).map_err(|e| format!("Invalid hex: {}", e))
}

// ─────────────────────────────────────────────────────────────────────────────
// Provider HTTP response types
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
    chunk_hash: String,
    chunk_data: Option<String>,
    proof: MerkleProofData,
}

#[derive(Debug, Deserialize)]
struct MerkleProofData {
    siblings: Vec<String>,
    path: Vec<bool>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Challenge types
// ─────────────────────────────────────────────────────────────────────────────

/// Fields parsed from ChallengeCreated event.
struct ChallengeEvent {
    deadline: u32,
    index: u16,
    bucket_id: u64,
    provider: Vec<u8>,
}

/// Full challenge details (event + on-chain storage).
struct ChallengeDetails {
    deadline: u32,
    index: u16,
    bucket_id: u64,
    mmr_root: H256,
    leaf_index: u64,
    chunk_index: u64,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse configuration
    let chain_ws = std::env::var("CHAIN_WS").unwrap_or_else(|_| "ws://127.0.0.1:9944".into());
    let provider_url =
        std::env::var("PROVIDER_URL").unwrap_or_else(|_| "http://127.0.0.1:3000".into());
    let seed = std::env::var("SEED").unwrap_or_else(|_| "//Alice".into());

    eprintln!("=== Challenge Watcher ===");
    eprintln!("Chain:        {}", chain_ws);
    eprintln!("Provider:     {}", provider_url);
    eprintln!("Seed:         {}", seed);

    // Create signing keypair using sp_core (for public key extraction)
    let sp_keypair =
        sr25519::Pair::from_string(&seed, None).map_err(|e| format!("Invalid seed: {:?}", e))?;
    let provider_account_bytes = sp_keypair.public().0;
    let provider_account_ss58 = sp_core::crypto::Ss58Codec::to_ss58check(&sp_keypair.public());

    // Create subxt-compatible keypair for signing transactions
    let keypair = subxt_signer::sr25519::Keypair::from_uri(
        &seed
            .parse()
            .map_err(|e| format!("Invalid seed URI: {:?}", e))?,
    )
    .map_err(|e| format!("Failed to create keypair: {:?}", e))?;

    eprintln!("Provider ID:  {}", provider_account_ss58);
    eprintln!();

    // Connect to chain
    let client = SubstrateClient::connect(&chain_ws).await?;
    let client = client.with_signer(keypair.clone());
    let http = HttpClient::new();

    eprintln!("Connected to chain. Watching for challenges...");

    // Subscribe to finalized blocks
    let mut block_stream = client
        .api()
        .blocks()
        .subscribe_finalized()
        .await
        .map_err(|e| format!("Failed to subscribe: {}", e))?;

    while let Some(block_result) = block_stream.next().await {
        let block = match block_result {
            Ok(b) => b,
            Err(e) => {
                eprintln!("Block stream error: {}", e);
                continue;
            }
        };

        let block_number = block.number();

        // Get events for this block
        let events = match block.events().await {
            Ok(e) => e,
            Err(e) => {
                eprintln!("  Failed to get events for block {}: {}", block_number, e);
                continue;
            }
        };

        // Look for ChallengeCreated events targeting our provider
        for event in events.iter() {
            let event = match event {
                Ok(e) => e,
                Err(_) => continue,
            };

            if event.pallet_name() != "StorageProvider"
                || event.variant_name() != "ChallengeCreated"
            {
                continue;
            }

            let challenge_event = match parse_challenge_event(&event) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!(
                        "  Block {}: Failed to parse ChallengeCreated event: {}",
                        block_number, e
                    );
                    continue;
                }
            };

            // Check if this challenge targets our provider
            if challenge_event.provider != provider_account_bytes {
                continue;
            }

            eprintln!(
                "  Block {}: Challenge detected! deadline={}, index={}, bucket={}",
                block_number,
                challenge_event.deadline,
                challenge_event.index,
                challenge_event.bucket_id,
            );

            // Query on-chain storage for full challenge details
            let challenge = match fetch_challenge_details(&client, &challenge_event).await {
                Ok(c) => c,
                Err(e) => {
                    eprintln!(
                        "  Failed to fetch challenge details ({},{}): {}",
                        challenge_event.deadline, challenge_event.index, e
                    );
                    continue;
                }
            };

            eprintln!(
                "  Challenge details: leaf={}, chunk={}, mmr_root=0x{}...",
                challenge.leaf_index,
                challenge.chunk_index,
                hex::encode(&challenge.mmr_root.as_bytes()[..4]),
            );

            // Respond to the challenge
            match respond_to_challenge(&client, &http, &provider_url, &challenge).await {
                Ok(()) => {
                    eprintln!(
                        "  Challenge ({},{}) defended successfully!",
                        challenge.deadline, challenge.index
                    );
                }
                Err(e) => {
                    eprintln!(
                        "  Failed to respond to challenge ({},{}): {}",
                        challenge.deadline, challenge.index, e
                    );
                }
            }
        }
    }

    eprintln!("Block stream ended");
    Ok(())
}

fn parse_challenge_event(
    event: &subxt::events::EventDetails<subxt::PolkadotConfig>,
) -> Result<ChallengeEvent, String> {
    let fields = event
        .field_values()
        .map_err(|e| format!("field_values() failed: {}", e))?;

    let challenge_id = fields
        .at("challenge_id")
        .ok_or("missing field 'challenge_id'")?;
    let deadline = challenge_id
        .at("deadline")
        .and_then(|v| v.as_u128())
        .ok_or("missing/invalid 'challenge_id.deadline'")? as u32;
    let index = challenge_id
        .at("index")
        .and_then(|v| v.as_u128())
        .ok_or("missing/invalid 'challenge_id.index'")? as u16;

    let bucket_id = fields
        .at("bucket_id")
        .and_then(|v| v.as_u128())
        .ok_or("missing/invalid 'bucket_id'")? as u64;

    let provider_val = fields.at("provider").ok_or("missing field 'provider'")?;
    let provider =
        extract_account_bytes(provider_val).ok_or("failed to extract provider account bytes")?;

    Ok(ChallengeEvent {
        deadline,
        index,
        bucket_id,
        provider,
    })
}

/// Fetch full challenge details from on-chain storage.
async fn fetch_challenge_details(
    client: &SubstrateClient,
    event: &ChallengeEvent,
) -> Result<ChallengeDetails, Box<dyn std::error::Error>> {
    let query = storage::challenges(event.deadline);
    let result = client
        .api()
        .storage()
        .at_latest()
        .await
        .map_err(|e| format!("Failed to get storage: {}", e))?
        .fetch(&query)
        .await
        .map_err(|e| format!("Failed to fetch challenges: {}", e))?;

    let thunk = result.ok_or("No challenges found at deadline")?;
    let value = thunk
        .to_value()
        .map_err(|e| format!("Failed to decode challenges: {}", e))?;

    // Value is a Vec<Challenge> — index into it
    let challenge_val = value
        .at(event.index as usize)
        .ok_or_else(|| format!("Challenge index {} not found", event.index))?;

    let mmr_root_val = challenge_val
        .at("mmr_root")
        .ok_or("Missing mmr_root in challenge")?;
    let mmr_root_bytes =
        extract_h256_bytes(mmr_root_val).ok_or("Failed to extract mmr_root bytes")?;
    let mmr_root = H256::from_slice(&mmr_root_bytes);

    let leaf_index = challenge_val
        .at("leaf_index")
        .and_then(|v| v.as_u128())
        .ok_or("Missing leaf_index")? as u64;

    let chunk_index = challenge_val
        .at("chunk_index")
        .and_then(|v| v.as_u128())
        .ok_or("Missing chunk_index")? as u64;

    Ok(ChallengeDetails {
        deadline: event.deadline,
        index: event.index,
        bucket_id: event.bucket_id,
        mmr_root,
        leaf_index,
        chunk_index,
    })
}

fn extract_account_bytes(val: &scale_value::Value<u32>) -> Option<Vec<u8>> {
    if let scale_value::ValueDef::Composite(composite) = &val.value {
        // Try direct extraction (32 byte values at this level)
        let bytes: Vec<u8> = composite
            .values()
            .filter_map(|v| v.as_u128().map(|n| n as u8))
            .collect();
        if bytes.len() == 32 {
            return Some(bytes);
        }
        // AccountId32 may be wrapped in extra composite layers — unwrap
        for inner in composite.values() {
            if let Some(result) = extract_account_bytes(inner) {
                return Some(result);
            }
        }
    }
    None
}

fn extract_h256_bytes(val: &scale_value::Value<u32>) -> Option<Vec<u8>> {
    extract_account_bytes(val)
}

async fn respond_to_challenge(
    client: &SubstrateClient,
    http: &HttpClient,
    provider_url: &str,
    challenge: &ChallengeDetails,
) -> Result<(), Box<dyn std::error::Error>> {
    // 1. Fetch MMR proof from provider
    let mmr_proof_resp: MmrProofResponse = http
        .get(format!("{}/mmr_proof", provider_url))
        .query(&[
            ("bucket_id", challenge.bucket_id.to_string()),
            ("leaf_index", challenge.leaf_index.to_string()),
        ])
        .send()
        .await?
        .json()
        .await?;

    let data_root = &mmr_proof_resp.leaf.data_root;

    // 2. Fetch chunk proof from provider
    let chunk_proof_resp: ChunkProofResponse = http
        .get(format!("{}/chunk_proof", provider_url))
        .query(&[
            ("data_root", data_root.to_string()),
            ("chunk_index", challenge.chunk_index.to_string()),
        ])
        .send()
        .await?
        .json()
        .await?;

    // 3. Get chunk data (included in chunk_proof response, or fetch from /node)
    let chunk_data = if let Some(ref b64_data) = chunk_proof_resp.chunk_data {
        BASE64.decode(b64_data)?
    } else {
        // Fallback: fetch from /node endpoint
        let node_resp: serde_json::Value = http
            .get(format!("{}/node", provider_url))
            .query(&[("hash", &chunk_proof_resp.chunk_hash)])
            .send()
            .await?
            .json()
            .await?;
        let b64_data = node_resp["data"]
            .as_str()
            .ok_or("Missing data in node response")?;
        BASE64.decode(b64_data)?
    };

    // 4. Convert HTTP response types to storage_primitives types
    let mmr_proof = convert_mmr_proof(&mmr_proof_resp)?;
    let chunk_proof = convert_merkle_proof(&chunk_proof_resp.proof)?;

    // 5. Submit respond_to_challenge extrinsic
    let signer = client.signer()?;
    let tx = extrinsics::respond_to_challenge_proof(
        (challenge.deadline, challenge.index),
        &chunk_data,
        &mmr_proof,
        &chunk_proof,
    );

    let tx_progress = client
        .api()
        .tx()
        .sign_and_submit_then_watch_default(&tx, signer)
        .await
        .map_err(|e| format!("Failed to submit tx: {}", e))?;

    tx_progress
        .wait_for_finalized_success()
        .await
        .map_err(|e| format!("Transaction failed: {}", e))?;

    Ok(())
}

fn convert_mmr_proof(resp: &MmrProofResponse) -> Result<MmrProof, String> {
    let peaks: Vec<H256> = resp
        .proof
        .peaks
        .iter()
        .map(|s| {
            let bytes = hex_decode(s)?;
            Ok(H256::from_slice(&bytes))
        })
        .collect::<Result<Vec<_>, String>>()?;

    let siblings: Vec<H256> = resp
        .proof
        .siblings
        .iter()
        .map(|s| {
            let bytes = hex_decode(s)?;
            Ok(H256::from_slice(&bytes))
        })
        .collect::<Result<Vec<_>, String>>()?;

    let data_root_bytes = hex_decode(&resp.leaf.data_root)?;
    let data_root = H256::from_slice(&data_root_bytes);

    Ok(MmrProof {
        peaks,
        leaf: MmrLeaf {
            data_root,
            data_size: resp.leaf.data_size,
            total_size: resp.leaf.total_size,
        },
        leaf_proof: MerkleProof {
            siblings,
            path: resp.proof.path.clone(),
        },
    })
}

fn convert_merkle_proof(resp: &MerkleProofData) -> Result<MerkleProof, String> {
    let siblings: Vec<H256> = resp
        .siblings
        .iter()
        .map(|s| {
            let bytes = hex_decode(s)?;
            Ok(H256::from_slice(&bytes))
        })
        .collect::<Result<Vec<_>, String>>()?;

    Ok(MerkleProof {
        siblings,
        path: resp.path.clone(),
    })
}
