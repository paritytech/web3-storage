//! Node startup and runtime orchestration.

use crate::{
    cli::{Cli, StorageMode, DEFAULT_PROVIDER_ID},
    create_router, CheckpointCoordinator, CheckpointCoordinatorConfig, CheckpointCoordinatorHandle,
    DiskStorage, ProviderState, ReplicaSyncCoordinator, ReplicaSyncCoordinatorConfig,
    ReplicaSyncCoordinatorHandle, Storage, StorageBackend,
};
use clap::Parser;
use std::sync::Arc;
use std::time::Duration;
use subxt::{dynamic::Value, OnlineClient, PolkadotConfig};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

/// Parse CLI arguments, initialize the node, and run the server.
pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "storage_provider_node=debug,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let cli = Cli::parse();

    // Create storage backend
    let storage: Arc<dyn StorageBackend> = match cli.storage.storage_mode {
        StorageMode::Inmemory => {
            tracing::info!("Using in-memory storage (data will be lost on restart)");
            Arc::new(Storage::new())
        }
        StorageMode::Disk => {
            tracing::info!(
                "Using persistent disk storage at: {}",
                cli.storage.storage_path.display()
            );
            Arc::new(DiskStorage::new(&cli.storage.storage_path)?)
        }
    };

    // Resolve provider identity
    let seed = cli.key.load_seed()?;
    let state = match &seed {
        Some(seed) => {
            let state = ProviderState::with_seed(storage, seed)?;
            tracing::info!("Signing enabled for account: {}", state.provider_id);
            Arc::new(state)
        }
        None => {
            let provider_id = cli
                .key
                .provider_id
                .clone()
                .unwrap_or_else(|| DEFAULT_PROVIDER_ID.to_string());
            tracing::warn!(
                "No --keyfile set, using --provider-id without signing: {}",
                provider_id
            );
            Arc::new(ProviderState::new(storage, provider_id))
        }
    };

    // Start optional background services (failures are non-fatal)
    let _checkpoint_handle = start_checkpoint_coordinator(&cli, state.clone()).await;
    let _replica_sync_handle = start_replica_sync_coordinator(&cli, state.clone()).await;

    // Sync on-chain multiaddr with actual bind address (requires signing key)
    if let Some(seed) = &seed {
        sync_multiaddr_on_chain(
            &cli.rpc.chain_rpc,
            seed,
            &state.provider_id,
            &cli.rpc.bind_addr,
        )
        .await;
    }

    tracing::info!("Starting storage provider node on {}", cli.rpc.bind_addr);

    let listener = tokio::net::TcpListener::bind(&cli.rpc.bind_addr).await?;
    let app = create_router(state);
    axum::serve(listener, app).await?;

    Ok(())
}

async fn start_checkpoint_coordinator(
    cli: &Cli,
    state: Arc<ProviderState>,
) -> Option<CheckpointCoordinatorHandle> {
    if !cli.checkpoint.enable_checkpoint_coordinator {
        return None;
    }

    let config = CheckpointCoordinatorConfig {
        chain_ws_url: cli.rpc.chain_rpc.clone(),
        ..Default::default()
    };

    let mut coordinator = CheckpointCoordinator::new(config, state);

    if let Err(e) = coordinator.connect().await {
        tracing::error!("Failed to connect checkpoint coordinator: {}", e);
        return None;
    }
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

async fn start_replica_sync_coordinator(
    cli: &Cli,
    state: Arc<ProviderState>,
) -> Option<ReplicaSyncCoordinatorHandle> {
    if !cli.replica_sync.enable_replica_sync {
        return None;
    }

    let config = ReplicaSyncCoordinatorConfig {
        chain_ws_url: cli.rpc.chain_rpc.clone(),
        poll_interval: Duration::from_secs(cli.replica_sync.replica_poll_interval),
        sync_timeout: Duration::from_secs(cli.replica_sync.replica_sync_timeout),
        max_concurrent_syncs: cli.replica_sync.replica_max_concurrent,
        auto_confirm: true,
    };

    let mut coordinator = ReplicaSyncCoordinator::new(config, state);

    if let Err(e) = coordinator.connect().await {
        tracing::error!("Failed to connect replica sync coordinator: {}", e);
        return None;
    }
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

/// Convert a bind address (e.g. "0.0.0.0:3333") to a multiaddr string (e.g. "/ip4/127.0.0.1/tcp/3333").
fn bind_addr_to_multiaddr(bind_addr: &str) -> String {
    let parts: Vec<&str> = bind_addr.split(':').collect();
    let (host, port) = if parts.len() == 2 {
        (parts[0], parts[1])
    } else {
        ("127.0.0.1", "3333")
    };
    // 0.0.0.0 isn't useful as a client-facing address
    let host = if host == "0.0.0.0" { "127.0.0.1" } else { host };
    format!("/ip4/{}/tcp/{}", host, port)
}

/// Ensure the on-chain multiaddr matches the actual bind address.
/// If the provider is registered and the multiaddr differs, submit an update transaction.
async fn sync_multiaddr_on_chain(chain_rpc: &str, seed: &str, provider_id: &str, bind_addr: &str) {
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

    // Extract multiaddr field from the ProviderInfo composite
    let current_multiaddr = {
        let decoded = provider_value.to_value();
        match &decoded {
            Ok(val) => {
                if let subxt::ext::scale_value::ValueDef::Composite(
                    subxt::ext::scale_value::Composite::Named(fields),
                ) = &val.value
                {
                    fields
                        .iter()
                        .find(|(name, _)| name == "multiaddr")
                        .and_then(|(_, v)| {
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
        tracing::info!("On-chain multiaddr matches bind address: {}", expected_multiaddr);
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
    let signer =
        subxt_signer::sr25519::Keypair::from_uri(&uri).expect("valid keypair from seed");

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
            Ok(_) => {
                tracing::info!("Multiaddr updated on-chain to: {}", expected_multiaddr)
            }
            Err(e) => tracing::error!("Multiaddr update tx failed: {}", e),
        },
        Err(e) => {
            tracing::error!("Failed to submit multiaddr update: {}", e);
        }
    }
}
