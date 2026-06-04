//! Basic S3 Client Usage Example
//!
//! Usage: cargo run --example basic_usage [chain_ws] [provider_url] [seed]

use s3_client::{PutObjectOptions, S3Client};
use sp_runtime::AccountId32;
use std::collections::HashMap;
use std::env;
use storage_client::{NegotiateRequest, ProviderClient};
use subxt_signer::sr25519::dev as dev_signer;

const DEFAULT_CHAIN_WS: &str = "ws://127.0.0.1:2222";
const DEFAULT_PROVIDER_URL: &str = "http://127.0.0.1:3333";
const DEFAULT_SEED: &str = "//Alice";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let args: Vec<String> = env::args().collect();
    let chain_url = args.get(1).map(|s| s.as_str()).unwrap_or(DEFAULT_CHAIN_WS);
    let provider_url = args
        .get(2)
        .map(|s| s.as_str())
        .unwrap_or(DEFAULT_PROVIDER_URL);
    let seed = args.get(3).map(|s| s.as_str()).unwrap_or(DEFAULT_SEED);

    println!("=== S3 Client Basic Usage Example ===\n");
    println!("Chain URL: {chain_url}");
    println!("Provider URL: {provider_url}");
    println!("Account: {seed}\n");

    println!("Creating S3 client...");
    let client = S3Client::new(chain_url, provider_url, seed).await?;
    println!("S3 client created successfully!\n");

    // Resolve owner/provider accounts. Owner derives from `seed`; provider is
    // the dev account the local node was started with (//Bob by default).
    let owner: AccountId32 = match seed {
        "//Alice" => dev_signer::alice().public_key().0.into(),
        "//Bob" => dev_signer::bob().public_key().0.into(),
        "//Charlie" => dev_signer::charlie().public_key().0.into(),
        "//Dave" => dev_signer::dave().public_key().0.into(),
        "//Eve" => dev_signer::eve().public_key().0.into(),
        "//Ferdie" => dev_signer::ferdie().public_key().0.into(),
        _ => return Err(format!("unsupported seed for this example: {seed}").into()),
    };
    // Discover the provider's on-chain account from its /info endpoint
    let provider = ProviderClient::fetch_provider_id(provider_url).await?;
    println!("  Provider account (from /info): {provider}");

    println!("Negotiating signed agreement terms with provider...");
    let signed = ProviderClient::negotiate_terms(
        provider_url,
        &NegotiateRequest {
            owner,
            max_bytes: 1_000_000_000, // 1 GB
            duration: 500,            // 500 blocks
            price_per_byte: 0,
            replica_params: None,
            bucket_id: None,
        },
    )
    .await?;
    println!(
        "Provider signed terms: nonce={}, valid_until={}\n",
        signed.terms.nonce, signed.terms.valid_until
    );

    let bucket_name = format!(
        "test-bucket-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs()
    );

    println!("Creating bucket: {bucket_name}");
    let bucket = client
        .create_bucket(&bucket_name, provider, signed.terms, signed.signature)
        .await?;
    println!("Bucket created:");
    println!("  S3 Bucket ID: {}", bucket.s3_bucket_id);
    println!("  Layer 0 Bucket ID: {}", bucket.layer0_bucket_id);
    println!();

    println!("Uploading object: hello.txt");
    let content = b"Hello, Web3 Storage!";
    let mut metadata = HashMap::new();
    metadata.insert("x-custom-key".to_string(), "custom-value".to_string());

    let put_result = client
        .put_object(
            &bucket_name,
            "hello.txt",
            content,
            PutObjectOptions {
                content_type: Some("text/plain".to_string()),
                metadata,
            },
        )
        .await?;

    println!("Object uploaded:");
    println!("  ETag: {}", put_result.etag);
    println!("  CID: {:?}", put_result.cid);
    println!("  Size: {} bytes", put_result.size);
    println!();

    println!("Downloading object: hello.txt");
    let get_result = client.get_object(&bucket_name, "hello.txt").await?;
    println!("Object downloaded:");
    println!("  Content: {}", String::from_utf8_lossy(&get_result.data));
    println!("  Size: {} bytes", get_result.size);
    println!();

    println!("Cleaning up...");
    client.delete_object(&bucket_name, "hello.txt").await?;
    println!("Object deleted");

    client.delete_bucket(&bucket_name).await?;
    println!("Bucket deleted");
    println!();

    println!("=== Example completed successfully! ===");
    Ok(())
}
