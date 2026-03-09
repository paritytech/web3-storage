#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    storage_provider_node::cli::run().await
}
