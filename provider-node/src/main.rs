//! Storage Provider Node binary.
//!
//! Run with: cargo run -p storage-provider-node
//!
//! Environment variables:
//! - SEED: Seed phrase or derivation path for signing (e.g., "//Alice")
//! - PROVIDER_ID: Provider account ID (only used if SEED is not set, no signing)
//! - BIND_ADDR: Address to bind to (default: 0.0.0.0:3000)
//! - CHAIN_RPC: WebSocket URL for the parachain (default: ws://127.0.0.1:9944)
//! - ENABLE_CHECKPOINT_COORDINATOR: Set to "true" to enable checkpoint coordination
//! - ENABLE_REPLICA_SYNC: Set to "true" to enable autonomous replica sync
//! - REPLICA_POLL_INTERVAL: Seconds between sync checks (default: 12)
//! - REPLICA_SYNC_TIMEOUT: Seconds before sync timeout (default: 300)
//! - REPLICA_MAX_CONCURRENT: Max concurrent bucket syncs (default: 3)

use std::sync::Arc;
use std::time::Duration;
use storage_provider_node::{
    create_router, CheckpointCoordinator, CheckpointCoordinatorConfig, ProviderState,
    ReplicaSyncCoordinator, ReplicaSyncCoordinatorConfig, Storage,
};
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
        tracing::warn!(
            "No SEED set, using PROVIDER_ID without signing capability: {}",
            provider_id
        );
        Arc::new(ProviderState::new(storage, provider_id))
    };

    // Build router
    let app = create_router(state.clone());

    // Optionally start checkpoint coordinator
    let _coordinator_handle = if std::env::var("ENABLE_CHECKPOINT_COORDINATOR")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false)
    {
        let chain_rpc =
            std::env::var("CHAIN_RPC").unwrap_or_else(|_| "ws://127.0.0.1:9944".to_string());

        let config = CheckpointCoordinatorConfig {
            chain_ws_url: chain_rpc,
            ..Default::default()
        };

        let mut coordinator = CheckpointCoordinator::new(config, state.clone());

        match coordinator.connect().await {
            Ok(()) => {
                tracing::info!("Checkpoint coordinator connected to chain");
                match coordinator.start(None).await {
                    Ok(handle) => {
                        tracing::info!("Checkpoint coordinator started");
                        Some(handle)
                    }
                    Err(e) => {
                        tracing::error!("Failed to start checkpoint coordinator: {}", e);
                        None
                    }
                }
            }
            Err(e) => {
                tracing::error!("Failed to connect checkpoint coordinator: {}", e);
                None
            }
        }
    } else {
        None
    };

    // Optionally start replica sync coordinator
    let _replica_sync_handle = if std::env::var("ENABLE_REPLICA_SYNC")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false)
    {
        let chain_rpc =
            std::env::var("CHAIN_RPC").unwrap_or_else(|_| "ws://127.0.0.1:9944".to_string());

        let poll_interval = std::env::var("REPLICA_POLL_INTERVAL")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(12);

        let sync_timeout = std::env::var("REPLICA_SYNC_TIMEOUT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(300);

        let max_concurrent = std::env::var("REPLICA_MAX_CONCURRENT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3);

        let config = ReplicaSyncCoordinatorConfig {
            chain_ws_url: chain_rpc,
            poll_interval: Duration::from_secs(poll_interval),
            sync_timeout: Duration::from_secs(sync_timeout),
            max_concurrent_syncs: max_concurrent,
            auto_confirm: true,
        };

        let mut coordinator = ReplicaSyncCoordinator::new(config, state.clone());

        match coordinator.connect().await {
            Ok(()) => {
                tracing::info!("Replica sync coordinator connected to chain");
                match coordinator.start(None).await {
                    Ok(handle) => {
                        tracing::info!("Replica sync coordinator started");
                        Some(handle)
                    }
                    Err(e) => {
                        tracing::error!("Failed to start replica sync coordinator: {}", e);
                        None
                    }
                }
            }
            Err(e) => {
                tracing::error!("Failed to connect replica sync coordinator: {}", e);
                None
            }
        }
    } else {
        None
    };

    // Get bind address
    let addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:3000".to_string());

    tracing::info!("Starting storage provider node on {}", addr);

    // Run server
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
