// SPDX-License-Identifier: GPL-3.0-only

//! Node startup and runtime orchestration.

use crate::{
    auth::{ChainMembershipResolver, MembershipCache},
    chain_state_coordinator::ChainStateCoordinator,
    cli::{Cli, StorageMode, DEFAULT_PROVIDER_ID},
    create_router,
    subxt_client::SubxtChainClient,
    ChainStateCoordinatorHandle, CheckpointCoordinator, CheckpointCoordinatorConfig,
    CheckpointCoordinatorHandle, DiskStorage, NonceStore, NullNonceStore, ProviderState,
    ReplicaSyncCoordinator, ReplicaSyncCoordinatorConfig, ReplicaSyncCoordinatorHandle, Storage,
    StorageBackend,
};
use clap::Parser;
use std::net::SocketAddr;
use std::str::FromStr;
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

    // Create storage backend and the associated nonce store (which follows the
    // same persistence mode so the nonce counter survives disk restarts).
    let (storage, nonce_store): (Arc<dyn StorageBackend>, Arc<dyn NonceStore>) =
        match cli.storage.storage_mode {
            StorageMode::Inmemory => {
                tracing::info!("Using in-memory storage (data will be lost on restart)");
                (Arc::new(Storage::new()), Arc::new(NullNonceStore))
            }
            StorageMode::Disk => {
                tracing::info!(
                    "Using persistent disk storage at: {}",
                    cli.storage.storage_path.display()
                );
                let disk = DiskStorage::new(&cli.storage.storage_path)?;
                let store = disk.nonce_store();
                (Arc::new(disk), store)
            }
        };

    // Resolve provider identity
    let seed = cli.key.load_seed()?;
    let state = match &seed {
        Some(seed) => {
            let mut state = ProviderState::with_seed(storage, seed)?;
            tracing::info!("Signing enabled for account: {}", state.provider_id);

            // Wire up auth if enabled
            if cli.auth.enable_auth {
                let resolver = ChainMembershipResolver::new(cli.rpc.chain_rpc.clone());
                let ttl = Duration::from_secs(cli.auth.auth_cache_ttl);
                let cache = MembershipCache::new(Box::new(resolver), ttl);
                state.auth_enabled = true;
                state.membership_cache = Some(Arc::new(cache));
                state.auth_max_skew = Duration::from_secs(cli.auth.auth_max_skew);
                tracing::info!(
                    "Auth enabled (cache_ttl={}s, max_skew={}s)",
                    cli.auth.auth_cache_ttl,
                    cli.auth.auth_max_skew
                );
            }

            // Install the nonce store before wrapping state in Arc. At this point
            // no other thread holds a reference to chain_state, so get_mut succeeds.
            if let Some(cs) = Arc::get_mut(&mut state.chain_state) {
                cs.nonce_store = nonce_store.clone();
            } else {
                tracing::error!(
                    "nonce store install skipped: chain_state Arc has multiple owners; \
                     disk-mode persistence is disabled for this run"
                );
            }
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

            let mut state = ProviderState::new(storage, provider_id);

            if cli.auth.enable_auth {
                let resolver = ChainMembershipResolver::new(cli.rpc.chain_rpc.clone());
                let ttl = Duration::from_secs(cli.auth.auth_cache_ttl);
                let cache = MembershipCache::new(Box::new(resolver), ttl);
                state.auth_enabled = true;
                state.membership_cache = Some(Arc::new(cache));
                state.auth_max_skew = Duration::from_secs(cli.auth.auth_max_skew);
                tracing::info!(
                    "Auth enabled (cache_ttl={}s, max_skew={}s)",
                    cli.auth.auth_cache_ttl,
                    cli.auth.auth_max_skew
                );
            }

            // Install the nonce store before wrapping state in Arc. At this point
            // no other thread holds a reference to chain_state, so get_mut succeeds.
            if let Some(cs) = Arc::get_mut(&mut state.chain_state) {
                cs.nonce_store = nonce_store.clone();
            } else {
                tracing::error!(
                    "nonce store install skipped: chain_state Arc has multiple owners; \
                     disk-mode persistence is disabled for this run"
                );
            }
            Arc::new(state)
        }
    };

    // Connect a single chain client shared by every coordinator. One
    // WebSocket connection and one signer (the provider's own account) back
    // all on-chain actions; coordinators each get a cheap clone. Requires a
    // signing key, so this is only available when a seed was provided.
    let chain_client = match &seed {
        Some(seed) => match SubxtChainClient::connect(&cli.rpc.chain_rpc, seed).await {
            Ok(client) => Some(client),
            Err(e) => {
                tracing::error!("Failed to connect chain client: {}", e);
                None
            }
        },
        None => None,
    };

    // Start optional background services (failures are non-fatal)
    let _chain_state_handle = start_chain_state_coordinator(&cli, state.clone());
    let checkpoint_handle =
        start_checkpoint_coordinator(&cli, chain_client.as_ref(), state.clone()).await;
    if let Some(ref handle) = checkpoint_handle {
        state.set_checkpoint_handle(handle);
    }
    let _replica_sync_handle =
        start_replica_sync_coordinator(&cli, chain_client.as_ref(), state.clone()).await;

    // Sync the on-chain multiaddr. Reuses the chain client connected above, so
    // this only runs when that connection succeeded (which also implies a
    // signing key was provided). Advertise the public multiaddr when configured,
    // otherwise derive one from the bind address.
    if let Some(chain_client) = &chain_client {
        chain_client
            .sync_multiaddr(
                &state.provider_id,
                &cli.rpc.bind_addr,
                cli.rpc.public_multiaddr.as_deref(),
            )
            .await;
    }

    tracing::info!("Starting storage provider node on {}", cli.rpc.bind_addr);

    let listener = tokio::net::TcpListener::bind(&cli.rpc.bind_addr).await?;
    let app = create_router(state);
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;

    Ok(())
}

/// Start the chain-state coordinator, which keeps `chain_state.current_block`
/// and `chain_state.provider_info` in sync with the chain.
///
/// Returns `None` only when the provider id isn't a valid account. The
/// coordinator itself never fails to start: it connects in the background and
/// retries with a backoff if the chain is unreachable, so `current_block` is
/// populated as soon as the chain comes up.
fn start_chain_state_coordinator(
    cli: &Cli,
    state: Arc<ProviderState>,
) -> Option<ChainStateCoordinatorHandle> {
    let provider_account = match sp_runtime::AccountId32::from_str(&state.provider_id) {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!(
                "chain-state coordinator: invalid provider SS58 '{}': {e:?}",
                state.provider_id
            );
            return None;
        }
    };

    let coordinator = ChainStateCoordinator::new(
        cli.rpc.chain_rpc.clone(),
        provider_account,
        state.chain_state.clone(),
    );

    tracing::info!("Chain-state coordinator started (retries until the chain is reachable)");
    Some(coordinator.start())
}

async fn start_checkpoint_coordinator(
    cli: &Cli,
    chain_client: Option<&SubxtChainClient>,
    state: Arc<ProviderState>,
) -> Option<CheckpointCoordinatorHandle> {
    if !cli.checkpoint.enable_checkpoint_coordinator {
        return None;
    }

    let chain_client = match chain_client {
        Some(c) => c.clone(),
        None => {
            tracing::error!(
                "Checkpoint coordinator needs a chain client (--keyfile + reachable chain). Disabled."
            );
            return None;
        }
    };

    let config = CheckpointCoordinatorConfig::default();

    let coordinator = CheckpointCoordinator::new(config, state, Box::new(chain_client));

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
    chain_client: Option<&SubxtChainClient>,
    state: Arc<ProviderState>,
) -> Option<ReplicaSyncCoordinatorHandle> {
    if !cli.replica_sync.enable_replica_sync {
        return None;
    }

    let chain_client = match chain_client {
        Some(c) => c.clone(),
        None => {
            tracing::error!(
                "Replica sync coordinator needs a chain client (--keyfile + reachable chain). Skipping."
            );
            return None;
        }
    };

    let config = ReplicaSyncCoordinatorConfig {
        poll_interval: Duration::from_secs(cli.replica_sync.replica_poll_interval),
        sync_timeout: Duration::from_secs(cli.replica_sync.replica_sync_timeout),
        max_concurrent_syncs: cli.replica_sync.replica_max_concurrent,
        auto_confirm: true,
    };

    let coordinator = ReplicaSyncCoordinator::new(config, state, Box::new(chain_client));

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
