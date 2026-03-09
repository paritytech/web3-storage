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
    let state = match cli.key.load_seed()? {
        Some(seed) => {
            let state = ProviderState::with_seed(storage, &seed)?;
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
                "No --keyfile or --dev set, using --provider-id without signing: {}",
                provider_id
            );
            Arc::new(ProviderState::new(storage, provider_id))
        }
    };

    // Start optional background services (failures are non-fatal)
    let _checkpoint_handle = start_checkpoint_coordinator(&cli, state.clone()).await;
    let _replica_sync_handle = start_replica_sync_coordinator(&cli, state.clone()).await;

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
