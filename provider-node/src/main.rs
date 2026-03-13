//! Storage Provider Node binary.
//!
//! Run with: cargo run -p storage-provider-node
//!
//! Environment variables:
//! - SEED: Seed phrase or derivation path for signing (e.g., "//Alice")
//! - PROVIDER_ID: Provider account ID (only used if SEED is not set, no signing)
//! - BIND_ADDR: Address to bind to (default: 0.0.0.0:3333)
//! - DATA_DIR: Directory for persistent data (S3 indices, etc.)
//! - CHAIN_RPC: WebSocket URL for the parachain (default: ws://127.0.0.1:2222)
//! - ENABLE_CHECKPOINT_COORDINATOR: Set to "true" to enable checkpoint coordination
//! - ENABLE_REPLICA_SYNC: Set to "true" to enable autonomous replica sync
//! - DISABLE_AUTO_ACCEPT: Set to "true" to disable auto-accepting agreement requests
//! - REPLICA_POLL_INTERVAL: Seconds between sync checks (default: 12)
//! - REPLICA_SYNC_TIMEOUT: Seconds before sync timeout (default: 300)
//! - REPLICA_MAX_CONCURRENT: Max concurrent bucket syncs (default: 3)

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use storage_provider_node::{
    create_router, AgreementCoordinator, AgreementCoordinatorConfig, CheckpointCoordinator,
    CheckpointCoordinatorConfig, FsIndexManager, ProviderState, ReplicaSyncCoordinator,
    ReplicaSyncCoordinatorConfig, S3IndexManager, Storage,
};
use subxt::{dynamic::Value, OnlineClient, PolkadotConfig};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

/// Convert a bind address (e.g. "0.0.0.0:3333") to a multiaddr string (e.g. "/ip4/127.0.0.1/tcp/3333").
/// Replaces 0.0.0.0 with 127.0.0.1 since 0.0.0.0 isn't useful as a client-facing address.
fn bind_addr_to_multiaddr(bind_addr: &str) -> String {
    let parts: Vec<&str> = bind_addr.split(':').collect();
    let (host, port) = if parts.len() == 2 {
        (parts[0], parts[1])
    } else {
        ("127.0.0.1", "3333")
    };
    let host = if host == "0.0.0.0" { "127.0.0.1" } else { host };
    format!("/ip4/{}/tcp/{}", host, port)
}

/// Ensure the on-chain multiaddr matches the actual bind address.
/// If the provider is registered and the multiaddr differs, submit an update transaction.
async fn sync_multiaddr_on_chain(
    chain_rpc: &str,
    seed: &str,
    provider_id: &str,
    bind_addr: &str,
) {
    let expected_multiaddr = bind_addr_to_multiaddr(bind_addr);

    let api = match OnlineClient::<PolkadotConfig>::from_url(chain_rpc).await {
        Ok(api) => api,
        Err(e) => {
            tracing::warn!("Could not connect to chain for multiaddr sync: {}", e);
            return;
        }
    };

    // Read current on-chain provider info
    let our_account: sp_core::crypto::AccountId32 =
        match sp_core::crypto::Ss58Codec::from_ss58check(provider_id) {
            Ok(a) => a,
            Err(_) => {
                tracing::warn!("Invalid provider SS58 address, skipping multiaddr sync");
                return;
            }
        };
    let our_bytes: [u8; 32] = our_account.into();

    let storage_query = subxt::dynamic::storage(
        "StorageProvider",
        "Providers",
        vec![Value::from_bytes(our_bytes)],
    );

    let result = match api.storage().at_latest().await {
        Ok(s) => s.fetch(&storage_query).await,
        Err(e) => {
            tracing::warn!("Failed to query storage for multiaddr sync: {}", e);
            return;
        }
    };

    let provider_value = match result {
        Ok(Some(v)) => v,
        Ok(None) => {
            tracing::info!("Provider not registered on chain yet, skipping multiaddr sync");
            return;
        }
        Err(e) => {
            tracing::warn!("Failed to fetch provider info: {}", e);
            return;
        }
    };

    // Extract multiaddr field from the provider info composite
    let current_multiaddr = {
        let decoded = provider_value.to_value();
        match &decoded {
            Ok(val) => {
                // ProviderInfo is a named composite; multiaddr is the first field
                if let subxt::ext::scale_value::ValueDef::Composite(
                    subxt::ext::scale_value::Composite::Named(fields),
                ) = &val.value
                {
                    fields.iter().find(|(name, _)| name == "multiaddr").and_then(|(_, v)| {
                        if let subxt::ext::scale_value::ValueDef::Composite(
                            subxt::ext::scale_value::Composite::Unnamed(bytes_vals),
                        ) = &v.value
                        {
                            let bytes: Vec<u8> = bytes_vals
                                .iter()
                                .filter_map(|b| {
                                    if let subxt::ext::scale_value::ValueDef::Primitive(
                                        subxt::ext::scale_value::Primitive::U128(n),
                                    ) = &b.value
                                    {
                                        Some(*n as u8)
                                    } else {
                                        None
                                    }
                                })
                                .collect();
                            Some(String::from_utf8_lossy(&bytes).to_string())
                        } else {
                            None
                        }
                    })
                } else {
                    None
                }
            }
            Err(_) => None,
        }
    };

    let current = match current_multiaddr {
        Some(m) => m,
        None => {
            tracing::warn!("Could not decode on-chain multiaddr, skipping sync");
            return;
        }
    };

    if current == expected_multiaddr {
        tracing::info!(
            "On-chain multiaddr matches bind address: {}",
            expected_multiaddr
        );
        return;
    }

    tracing::info!(
        "On-chain multiaddr mismatch: chain=\"{}\" actual=\"{}\", updating...",
        current,
        expected_multiaddr
    );

    // Create signer from seed
    let uri: subxt_signer::SecretUri = match seed.parse() {
        Ok(u) => u,
        Err(e) => {
            tracing::error!("Failed to parse seed for multiaddr update: {:?}", e);
            return;
        }
    };
    let signer = subxt_signer::sr25519::Keypair::from_uri(&uri)
        .expect("valid keypair from seed");

    // Build update_provider_multiaddr transaction
    let multiaddr_bytes = expected_multiaddr.as_bytes().to_vec();
    let tx = subxt::dynamic::tx(
        "StorageProvider",
        "update_provider_multiaddr",
        vec![Value::from_bytes(multiaddr_bytes)],
    );

    match api
        .tx()
        .sign_and_submit_then_watch_default(&tx, &signer)
        .await
    {
        Ok(progress) => match progress.wait_for_finalized_success().await {
            Ok(_) => tracing::info!(
                "Multiaddr updated on-chain to: {}",
                expected_multiaddr
            ),
            Err(e) => tracing::error!("Multiaddr update tx failed: {}", e),
        },
        Err(e) => {
            tracing::error!("Failed to submit multiaddr update: {}", e);
        }
    }
}

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

    // Create S3 index manager with optional persistence
    let s3_index = if let Ok(data_dir) = std::env::var("DATA_DIR") {
        let path = PathBuf::from(&data_dir);
        match S3IndexManager::with_persistence(&path) {
            Ok(manager) => {
                match manager.load_from_disk() {
                    Ok(count) => {
                        if count > 0 {
                            tracing::info!("Loaded {} S3 bucket indices from disk", count);
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Failed to load S3 indices from disk: {}", e);
                    }
                }
                tracing::info!("S3 index persistence enabled at {}", data_dir);
                manager
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to create S3 index persistence at {}: {}",
                    data_dir,
                    e
                );
                S3IndexManager::new()
            }
        }
    } else {
        tracing::info!("No DATA_DIR set, S3 indices will be in-memory only");
        S3IndexManager::new()
    };

    // Create FS index manager with optional persistence
    let fs_index = if let Ok(data_dir) = std::env::var("DATA_DIR") {
        let path = PathBuf::from(&data_dir);
        match FsIndexManager::with_persistence(&path) {
            Ok(manager) => {
                match manager.load_from_disk() {
                    Ok(count) => {
                        if count > 0 {
                            tracing::info!("Loaded {} FS drive indices from disk", count);
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Failed to load FS indices from disk: {}", e);
                    }
                }
                tracing::info!("FS index persistence enabled at {}", data_dir);
                manager
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to create FS index persistence at {}: {}",
                    data_dir,
                    e
                );
                FsIndexManager::new()
            }
        }
    } else {
        FsIndexManager::new()
    };

    // Create provider state - prefer SEED over PROVIDER_ID
    let state = if let Ok(seed) = std::env::var("SEED") {
        match ProviderState::with_seed_and_index(storage, &seed, s3_index, fs_index) {
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
        Arc::new(ProviderState::new_with_index(
            storage,
            provider_id,
            s3_index,
            fs_index,
        ))
    };

    // Build router
    let app = create_router(state.clone());

    // Optionally start checkpoint coordinator
    let _coordinator_handle = if std::env::var("ENABLE_CHECKPOINT_COORDINATOR")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false)
    {
        let chain_rpc =
            std::env::var("CHAIN_RPC").unwrap_or_else(|_| "ws://127.0.0.1:2222".to_string());

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
            std::env::var("CHAIN_RPC").unwrap_or_else(|_| "ws://127.0.0.1:2222".to_string());

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

    // Start agreement coordinator by default when SEED is set (signing available)
    // Disable with DISABLE_AUTO_ACCEPT=true
    let _agreement_handle = if state.keypair.is_some()
        && !std::env::var("DISABLE_AUTO_ACCEPT")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false)
    {
        let chain_rpc =
            std::env::var("CHAIN_RPC").unwrap_or_else(|_| "ws://127.0.0.1:2222".to_string());
        let seed = std::env::var("SEED").ok();

        let config = AgreementCoordinatorConfig {
            chain_ws_url: chain_rpc,
            seed,
            ..Default::default()
        };

        let mut coordinator = AgreementCoordinator::new(config, state.clone());

        match coordinator.connect().await {
            Ok(()) => {
                tracing::info!("Agreement coordinator connected to chain");
                match coordinator.start().await {
                    Ok(handle) => {
                        tracing::info!("Agreement coordinator started");
                        Some(handle)
                    }
                    Err(e) => {
                        tracing::error!("Failed to start agreement coordinator: {}", e);
                        None
                    }
                }
            }
            Err(e) => {
                tracing::error!("Failed to connect agreement coordinator: {}", e);
                None
            }
        }
    } else {
        None
    };

    // Get bind address
    let addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:3333".to_string());

    // Sync on-chain multiaddr with actual bind address (requires SEED for signing)
    if let Ok(seed) = std::env::var("SEED") {
        let chain_rpc =
            std::env::var("CHAIN_RPC").unwrap_or_else(|_| "ws://127.0.0.1:2222".to_string());
        sync_multiaddr_on_chain(&chain_rpc, &seed, &state.provider_id, &addr).await;
    }

    tracing::info!("Starting storage provider node on {}", addr);

    // Run server
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
