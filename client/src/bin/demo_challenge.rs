//! Demo to challenge a storage provider using off-chain commitment.
//!
//! Usage: cargo run --release -p storage-client --bin demo_challenge -- \
//!          <chain_ws_url> <bucket_id> <provider_account> <leaf_index> <chunk_index> \
//!          <mmr_root> <start_seq> <provider_signature>
//!
//! Example:
//!   cargo run --release -p storage-client --bin demo_challenge -- \
//!     ws://127.0.0.1:9944 1 5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY \
//!     0 0 0xabc... 0 0xdef...

use sp_core::H256;
use storage_client::{ChallengerClient, ClientConfig};

fn hex_decode(s: &str) -> Result<Vec<u8>, String> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    hex::decode(s).map_err(|e| format!("Invalid hex: {}", e))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();

    // Parse arguments
    let chain_ws_url = args
        .get(1)
        .map(|s| s.as_str())
        .unwrap_or("ws://127.0.0.1:9944");

    let bucket_id: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1);

    let provider_account = args
        .get(3)
        .cloned()
        .unwrap_or_else(|| "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY".to_string());

    let leaf_index: u64 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(0);

    let chunk_index: u64 = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(0);

    // Off-chain challenge parameters
    let mmr_root_hex = args.get(6).map(|s| s.as_str());
    let start_seq: u64 = args.get(7).and_then(|s| s.parse().ok()).unwrap_or(0);
    let provider_signature_hex = args.get(8).map(|s| s.as_str());

    println!("=== Storage Provider Challenge Demo ===\n");
    println!("Chain:           {}", chain_ws_url);
    println!("Bucket ID:       {}", bucket_id);
    println!("Provider:        {}", provider_account);
    println!("Leaf Index:      {}", leaf_index);
    println!("Chunk Index:     {}", chunk_index);

    let use_offchain = mmr_root_hex.is_some() && provider_signature_hex.is_some();
    if use_offchain {
        println!("MMR Root:        {}", mmr_root_hex.unwrap());
        println!("Start Seq:       {}", start_seq);
        println!(
            "Provider Sig:    {}...",
            &provider_signature_hex.unwrap()[..20.min(provider_signature_hex.unwrap().len())]
        );
        println!("Challenge Mode:  Off-chain (using provider signature)");
    } else {
        println!("Challenge Mode:  On-chain checkpoint");
    }
    println!();

    // Create challenger client (using bob as the challenger)
    let config = ClientConfig {
        chain_ws_url: chain_ws_url.to_string(),
        provider_urls: vec![],
        timeout_secs: 30,
        enable_retries: true,
    };

    // The challenger account (bob in this demo)
    let challenger_account = "5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty".to_string();
    let mut challenger = ChallengerClient::new(config, challenger_account)?;

    println!("Connecting to chain...");
    challenger.connect().await?;
    challenger.set_dev_signer("bob")?;
    println!("Connected!\n");

    // Submit the challenge
    println!("Submitting challenge...");

    let result = if use_offchain {
        // Parse MMR root
        let mmr_root_bytes = hex_decode(mmr_root_hex.unwrap())?;
        let mmr_root = H256::from_slice(&mmr_root_bytes);

        // Parse provider signature
        let provider_signature = hex_decode(provider_signature_hex.unwrap())?;

        challenger
            .challenge_offchain(
                bucket_id,
                provider_account.clone(),
                mmr_root,
                start_seq,
                leaf_index,
                chunk_index,
                provider_signature,
            )
            .await
    } else {
        challenger
            .challenge_checkpoint(bucket_id, provider_account.clone(), leaf_index, chunk_index)
            .await
    };

    match result {
        Ok(challenge_id) => {
            println!("Challenge created successfully!");
            println!(
                "  Challenge ID: (deadline: {}, index: {})",
                challenge_id.deadline, challenge_id.index
            );
            println!();
            println!("The provider must respond before the deadline or face penalties.");
            println!("If the provider fails to respond, the challenger earns a reward.");
        }
        Err(e) => {
            eprintln!("Challenge failed: {}", e);
            eprintln!();
            if use_offchain {
                eprintln!("Common reasons for off-chain challenge failure:");
                eprintln!("  - Invalid provider signature");
                eprintln!("  - MMR root doesn't match signed commitment");
                eprintln!("  - Provider doesn't have an agreement for this bucket");
            } else {
                eprintln!("Common reasons for checkpoint challenge failure:");
                eprintln!("  - No checkpoint exists for this bucket yet");
                eprintln!("  - The provider doesn't have an agreement for this bucket");
            }
            eprintln!("  - Invalid leaf_index or chunk_index");
            eprintln!("  - Challenger doesn't have enough balance for deposit");
            return Err(e.into());
        }
    }

    println!("\nDemo complete!");
    Ok(())
}
