//! Basic S3 Client Usage Example

use s3_client::{PutObjectOptions, S3Client};
use std::collections::HashMap;
use std::env;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let chain_url = env::var("CHAIN_WS").unwrap_or_else(|_| "ws://127.0.0.1:2222".to_string());
    let provider_url =
        env::var("PROVIDER_URL").unwrap_or_else(|_| "http://127.0.0.1:3333".to_string());
    let seed = env::var("SEED").unwrap_or_else(|_| "//Alice".to_string());

    println!("=== S3 Client Basic Usage Example ===\n");
    println!("Chain URL: {}", chain_url);
    println!("Provider URL: {}", provider_url);
    println!("Account: {}\n", seed);

    println!("Creating S3 client...");
    let client = S3Client::new(&chain_url, &provider_url, &seed).await?;
    println!("S3 client created successfully!\n");

    let bucket_name = format!(
        "test-bucket-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs()
    );

    println!("Creating bucket: {}", bucket_name);
    let bucket = client.create_bucket(&bucket_name).await?;
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
