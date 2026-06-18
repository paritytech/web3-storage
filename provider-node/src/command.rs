// SPDX-License-Identifier: GPL-3.0-only

//! Node startup and runtime orchestration.

use crate::{
    auth::{ChainMembershipResolver, MembershipCache},
    chain_state_coordinator::ChainStateCoordinator,
    cli::{Cli, StorageMode, DEFAULT_PROVIDER_ID},
    create_router,
    subxt_client::SubxtChainClient,
    ChainStateCoordinatorHandle, CheckpointCoordinator, CheckpointCoordinatorConfig,
    CheckpointCoordinatorHandle, DiskStorage, ProviderState, ReplicaSyncCoordinator,
    ReplicaSyncCoordinatorConfig, ReplicaSyncCoordinatorHandle, Storage, StorageBackend,
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

            // The background reconciler bootstraps the nonce counter and
            // publishes `provider_info`. `request_timeout` is a runtime constant
            // the reconciler doesn't touch, so read it once here for the
            // `/negotiate` replay window.
            state.request_timeout = setup_request_timeout(&cli).await;

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

            Arc::new(state)
        }
    };

    // Keep the node's view of its own on-chain registration current. The chain
    // is the source of truth for the provider's settings and replay window, so
    // we poll it in the background rather than reading once at startup. This
    // makes registration order irrelevant: the provider can register *after*
    // the node is already serving (the node picks it up with no restart), and
    // later settings changes are reflected too. Only meaningful when we can
    // sign, so gate on having a key.
    if seed.is_some() {
        let interval = Duration::from_secs(cli.rpc.reconcile_interval_secs);
        spawn_chain_reconciler(cli.rpc.chain_rpc.clone(), interval, state.clone());
    }

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
    let _chain_state_handle = start_chain_state_coordinator(&cli, state.clone()).await;
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

/// Bootstrap `chain_state.current_block` from the latest block and start the
/// chain-state coordinator.
///
/// Failure to connect is logged and non-fatal — the coordinator simply won't
/// run and `current_block` will remain 0.
async fn start_chain_state_coordinator(
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

    match coordinator.start().await {
        Ok(handle) => {
            tracing::info!("Chain-state coordinator started");
            Some(handle)
        }
        Err(e) => {
            tracing::warn!(
                "Chain-state coordinator failed to start: {e}; current_block will not update"
            );
            None
        }
    }
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

/// Spawn a background task that keeps the node's view of its own on-chain
/// registration current.
///
/// The chain is the source of truth for the provider's settings
/// ([`ProviderState::provider_info`]) and replay window
/// ([`ProviderState::nonce_counter`]). Instead of reading these once at
/// startup — which would miss a provider that registers *after* the node is up
/// and never notice later settings changes — we poll every `interval`. The
/// first poll runs immediately, so an already-registered provider is picked up
/// right away.
///
/// All failures are non-fatal: a chain hiccup or an unregistered provider just
/// means we keep the previous view and retry on the next tick.
fn spawn_chain_reconciler(chain_rpc: String, interval: Duration, state: Arc<ProviderState>) {
    let provider_account = match sp_runtime::AccountId32::from_str(&state.provider_id) {
        Ok(account) => account,
        Err(e) => {
            tracing::warn!(
                "Provider id {} is not a valid account ({e:?}); skipping on-chain \
                 reconciliation. Signing endpoints will stay unavailable.",
                state.provider_id
            );
            return;
        }
    };

    tokio::spawn(async move {
        // Tracks the last observed registration status so we only log on
        // transitions (registered <-> unregistered) rather than every tick.
        let mut was_registered = false;
        loop {
            reconcile_once(&chain_rpc, &provider_account, &state, &mut was_registered).await;
            tokio::time::sleep(interval).await;
        }
    });
}

/// Perform a single reconciliation pass against the chain. Best-effort: any
/// error leaves the existing view untouched and is retried on the next tick.
async fn reconcile_once(
    chain_rpc: &str,
    provider_account: &sp_runtime::AccountId32,
    state: &ProviderState,
    was_registered: &mut bool,
) {
    let provider_id = &state.provider_id;

    let client = storage_client::ProviderClient::new(
        storage_client::ClientConfig {
            chain_ws_url: chain_rpc.to_string(),
            ..Default::default()
        },
        provider_id.clone(),
    );
    let mut client = match client {
        Ok(client) => client,
        Err(e) => {
            tracing::debug!("reconciler: failed to build provider client: {e:?}");
            return;
        }
    };
    if let Err(e) = client.connect().await {
        tracing::debug!("reconciler: failed to connect to chain: {e:?}");
        return;
    }

    match client.get_provider_info(provider_account).await {
        Ok(Some(info)) => {
            // Align the nonce counter with the chain's replay window *before*
            // publishing `provider_info`. `/negotiate` gates on `provider_info`
            // being `Some`, so once it is visible the counter is guaranteed to
            // be bootstrapped (see the defensive check in `negotiate_terms`).
            match storage_client::ProviderClient::fetch_replay_hsn(chain_rpc, provider_account)
                .await
            {
                Ok(Some(hsn)) => state.nonce_counter.bootstrap_from_hsn(hsn),
                Ok(None) => {
                    // Registered but no replay state is a transient/inconsistent
                    // view (registration inserts both atomically). Defer.
                    tracing::debug!(
                        "reconciler: provider {provider_id} registered but replay state \
                         missing; deferring to next tick"
                    );
                    return;
                }
                Err(e) => {
                    tracing::debug!("reconciler: failed to fetch replay hsn: {e:?}");
                    return;
                }
            }

            if let Ok(mut guard) = state.provider_info.write() {
                *guard = Some(info.clone());
            }

            if !*was_registered {
                *was_registered = true;
                tracing::info!(
                    "Provider {provider_id} is registered on chain: price_per_byte={}, \
                     duration=[{}, {}], max_capacity={}, accepting_primary={}. Signing \
                     endpoints are now available.",
                    info.price_per_byte,
                    info.min_duration,
                    info.max_duration,
                    info.max_capacity,
                    info.accepting_primary,
                );
            }
        }
        Ok(None) => {
            if let Ok(mut guard) = state.provider_info.write() {
                *guard = None;
            }
            if *was_registered {
                *was_registered = false;
                tracing::warn!(
                    "Provider {provider_id} is no longer registered on chain; signing \
                     endpoints are unavailable until it is re-registered."
                );
            } else {
                tracing::debug!("reconciler: provider {provider_id} not registered on chain yet");
            }
        }
        Err(e) => {
            tracing::debug!("reconciler: failed to fetch provider info: {e:?}");
        }
    }
}

/// Read the `StorageProvider::RequestTimeout` runtime constant from the chain.
///
/// Returns 0 on any failure (warn and continue) — the negotiate handler
/// will detect the zero and return a 503 until the node is restarted with a
/// reachable chain.
async fn setup_request_timeout(cli: &Cli) -> u32 {
    match storage_client::ProviderClient::fetch_request_timeout(&cli.rpc.chain_rpc).await {
        Ok(Some(timeout)) => {
            tracing::info!("Bootstrapped request_timeout from chain: {timeout} blocks");
            timeout
        }
        Ok(None) => {
            tracing::warn!(
                "RequestTimeout constant absent from node metadata; request_timeout set to 0"
            );
            0
        }
        Err(e) => {
            tracing::warn!(
                "Failed to read RequestTimeout from chain: {e}; request_timeout set to 0"
            );
            0
        }
    }
}
