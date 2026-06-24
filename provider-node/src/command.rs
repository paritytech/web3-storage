// SPDX-License-Identifier: GPL-3.0-only

//! Node startup and runtime orchestration.

use crate::{
    auth::{ChainMembershipResolver, MembershipCache},
    chain_state_coordinator::ChainStateCoordinator,
    cli::{Cli, StorageMode, DEFAULT_PROVIDER_ID},
    create_router,
    subxt_client::SubxtChainClient,
    ChainStateCoordinatorHandle, ChallengeResponder, ChallengeResponderConfig, ChallengeResponderHandle, CheckpointCoordinator,
    CheckpointCoordinatorConfig, CheckpointCoordinatorHandle, DiskStorage, NonceStore, NullNonceStore, ProviderState,
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
    let mut state = match &seed {
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

            state
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

            state
        }
    };

    // Install the nonce store before sharing `state` across coordinators: while
    // it is still solely owned here, `chain_state`'s Arc has a single owner, so
    // the in-place install succeeds.
    state.set_nonce_store(nonce_store);

    let state = Arc::new(state);

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
    let _challenge_responder_handle =
        start_challenge_responder(&cli, chain_client.as_ref(), state.clone()).await;

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

async fn start_challenge_responder(
    cli: &Cli,
    chain_client: Option<&SubxtChainClient>,
    state: Arc<ProviderState>,
) -> Option<ChallengeResponderHandle> {
    if !cli.challenge_responder.enable_challenge_responder {
        return None;
    }

    let chain_client = match chain_client {
        Some(c) => c.clone(),
        None => {
            tracing::error!(
                "Challenge responder needs a chain client (--keyfile + reachable chain). Skipping."
            );
            return None;
        }
    };

    let config = ChallengeResponderConfig {
        poll_interval: Duration::from_secs(cli.challenge_responder.challenge_poll_interval),
        ..Default::default()
    };

    let responder = ChallengeResponder::new(config, state, Box::new(chain_client));

    match responder.start(None).await {
        Ok(handle) => {
            tracing::info!("Challenge responder started — auto-responding to challenges");
            Some(handle)
        }
        Err(e) => {
            tracing::error!("Failed to start challenge responder: {}", e);
            None
        }
    }
}
