//! Storage Provider Node binary.
//!
//! Run with: cargo run -p storage-provider-node
//!
//! Environment variables:
//! - SEED: Seed phrase or derivation path for signing (e.g., "//Alice")
//! - PROVIDER_ID: Provider account ID (only used if SEED is not set, no signing)
//! - BIND_ADDR: Address to bind to (default: 0.0.0.0:3000)

use std::sync::Arc;
use storage_provider_node::{create_router, ProviderState, Storage};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "storage_provider_node=debug,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Create storage backend
    let storage = Arc::new(Storage::new());

    // Create provider state - prefer SEED over PROVIDER_ID
    let state = if let Ok(seed) = std::env::var("SEED") {
        match ProviderState::with_seed(storage, &seed) {
            Ok(state) => {
                tracing::info!("Signing enabled for account: {}", state.provider_id);
                Arc::new(state)
            }
            Err(e) => {
                tracing::error!("Failed to create keypair from SEED: {}", e);
                std::process::exit(1);
            }
        }
    } else {
        let provider_id = std::env::var("PROVIDER_ID")
            .unwrap_or_else(|_| "0x0000000000000000000000000000000000000000".to_string());
        tracing::warn!("No SEED set, using PROVIDER_ID without signing capability: {}", provider_id);
        Arc::new(ProviderState::new(storage, provider_id))
    };

    // Build router
    let app = create_router(state);

    // Get bind address
    let addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:3000".to_string());

    tracing::info!("Starting storage provider node on {}", addr);

    // Run server
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
