//! Complete workflow example demonstrating all client types.
//!
//! This example shows:
//! 1. Provider registration
//! 2. Bucket creation by admin
//! 3. Agreement request and acceptance
//! 4. Data upload by storage user
//! 5. Checkpoint creation
//! 6. Challenge by third party
//! 7. Challenge response by provider

use storage_client::{
    AdminClient, ChallengerClient, ChunkingStrategy, ClientConfig, ProviderClient,
    StorageUserClient,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing for logs
    tracing_subscriber::fmt::init();

    println!("=== Scalable Web3 Storage Complete Workflow ===\n");

    // Configuration
    let config = ClientConfig {
        chain_ws_url: "ws://localhost:9944".to_string(),
        provider_urls: vec!["http://localhost:3000".to_string()],
        timeout_secs: 30,
        enable_retries: true,
    };

    // ═════════════════════════════════════════════════════════════════════════
    // Step 1: Provider Registration
    // ═════════════════════════════════════════════════════════════════════════
    println!("📦 Step 1: Provider Registration");

    let provider_account = "5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty";
    let provider_client = ProviderClient::new(config.clone(), provider_account.to_string())?;

    println!("  Registering provider {}...", provider_account);
    provider_client
        .register(
            "/ip4/203.0.113.1/tcp/3000".to_string(),
            vec![0u8; 32], // Mock public key
            10_000_000_000_000, // 10 tokens stake
        )
        .await?;
    println!("  ✓ Provider registered with 10 tokens stake\n");

    // ═════════════════════════════════════════════════════════════════════════
    // Step 2: Bucket Creation by Admin
    // ═════════════════════════════════════════════════════════════════════════
    println!("🗂️  Step 2: Bucket Creation");

    let admin_account = "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY";
    let admin_client = AdminClient::new(config.clone(), admin_account.to_string())?;

    println!("  Creating bucket with min_providers=1...");
    let bucket_id = admin_client.create_bucket(1).await?;
    println!("  ✓ Bucket created with ID: {}\n", bucket_id);

    // ═════════════════════════════════════════════════════════════════════════
    // Step 3: Agreement Request and Acceptance
    // ═════════════════════════════════════════════════════════════════════════
    println!("🤝 Step 3: Storage Agreement");

    println!("  Admin requesting storage agreement...");
    admin_client
        .request_agreement(
            bucket_id,
            provider_account.to_string(),
            10 * 1024 * 1024 * 1024, // 10 GB
            100_000,                  // ~2 weeks at 6 sec blocks
            5_000_000_000_000,        // 5 tokens payment
            None,                     // Primary (not replica)
        )
        .await?;
    println!("  ✓ Agreement requested: 10 GB for 100,000 blocks");

    println!("  Provider accepting agreement...");
    provider_client.accept_agreement(bucket_id).await?;
    println!("  ✓ Agreement accepted!\n");

    // ═════════════════════════════════════════════════════════════════════════
    // Step 4: Data Upload by Storage User
    // ═════════════════════════════════════════════════════════════════════════
    println!("⬆️  Step 4: Data Upload");

    let user_client = StorageUserClient::new(config.clone())?;

    let data = b"Hello, decentralized world! This is my important data that \
                 needs to be stored reliably across multiple providers. \
                 It's content-addressed and cryptographically verified!";

    println!("  Uploading {} bytes...", data.len());
    let data_root = user_client
        .upload(bucket_id, data, ChunkingStrategy::default())
        .await?;
    println!(
        "  ✓ Data uploaded with root: 0x{}",
        hex::encode(data_root.as_bytes())
    );

    println!("  Committing to chain...");
    let commitment = user_client.commit(bucket_id, vec![data_root]).await?;
    println!("  ✓ Committed with MMR root: {}\n", commitment.mmr_root);

    // ═════════════════════════════════════════════════════════════════════════
    // Step 5: Data Verification
    // ═════════════════════════════════════════════════════════════════════════
    println!("✅ Step 5: Data Verification");

    println!("  Downloading data...");
    let retrieved_data = user_client
        .download(&data_root, 0, data.len() as u64)
        .await?;
    println!("  ✓ Downloaded {} bytes", retrieved_data.len());

    if retrieved_data == data {
        println!("  ✓ Data integrity verified!");
    } else {
        println!("  ✗ Data mismatch!");
    }
    println!();

    // ═════════════════════════════════════════════════════════════════════════
    // Step 6: Challenge by Third Party
    // ═════════════════════════════════════════════════════════════════════════
    println!("🎯 Step 6: Data Integrity Challenge");

    let challenger_account = "5DAAnrj7VHTznn2AWBemMuyBwZWs6FNFjdyVXUeYum3PTXFy";
    let challenger_client =
        ChallengerClient::new(config.clone(), challenger_account.to_string())?;

    println!("  Challenger analyzing provider...");
    let analysis = challenger_client
        .analyze_provider(bucket_id, provider_account.to_string())
        .await?;
    println!("  Provider reputation: {}", analysis.reputation);
    println!("  Recommendation: {:?}", analysis.recommendation);

    println!("  Creating challenge...");
    let challenge_id = challenger_client
        .challenge_checkpoint(
            bucket_id,
            provider_account.to_string(),
            0,  // leaf_index
            0,  // chunk_index
        )
        .await?;
    println!(
        "  ✓ Challenge created: deadline={}, index={}",
        challenge_id.deadline, challenge_id.index
    );
    println!("  Provider must respond within challenge timeout!\n");

    // ═════════════════════════════════════════════════════════════════════════
    // Step 7: Challenge Response by Provider
    // ═════════════════════════════════════════════════════════════════════════
    println!("🛡️  Step 7: Provider Response");

    println!("  Provider fetching challenged data from local storage...");
    // In a real scenario, provider would:
    // 1. Load chunk from local storage
    // 2. Generate Merkle proof
    // 3. Generate MMR proof
    // 4. Submit response on-chain

    println!("  ✓ Provider would respond with proofs\n");

    // ═════════════════════════════════════════════════════════════════════════
    // Step 8: Monitoring & Analytics
    // ═════════════════════════════════════════════════════════════════════════
    println!("📊 Step 8: Monitoring & Analytics");

    println!("  Provider statistics:");
    let provider_stats = provider_client.get_stats().await?;
    println!("    - Total agreements: {}", provider_stats.agreements_total);
    println!(
        "    - Challenges received: {}",
        provider_stats.challenges_received
    );
    println!("    - Reputation: {}/100", provider_stats.reputation);

    println!("\n  Challenger statistics:");
    let challenge_stats = challenger_client.get_challenge_stats().await?;
    println!(
        "    - Total challenges: {}",
        challenge_stats.total_challenges
    );
    println!(
        "    - Success rate: {:.1}%",
        if challenge_stats.total_challenges > 0 {
            (challenge_stats.successful_challenges as f64
                / challenge_stats.total_challenges as f64)
                * 100.0
        } else {
            0.0
        }
    );
    println!("    - Total earnings: {} tokens", challenge_stats.total_earnings);

    println!("\n=== Workflow Complete ===");
    println!("\n💡 Key Takeaways:");
    println!("  • Providers stake collateral and offer storage");
    println!("  • Admins create buckets and manage agreements");
    println!("  • Users upload data with cryptographic guarantees");
    println!("  • Challengers enforce accountability");
    println!("  • All operations are verifiable on-chain");

    Ok(())
}
