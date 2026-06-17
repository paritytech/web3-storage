// SPDX-License-Identifier: GPL-3.0-only

//! Node startup and runtime orchestration.

use crate::{
    auth::{ChainMembershipResolver, MembershipCache},
    cli::{Cli, StorageMode, DEFAULT_PROVIDER_ID},
    create_router,
    subxt_client::SubxtChainClient,
    ChallengeResponder, ChallengeResponderConfig, ChallengeResponderHandle, CheckpointCoordinator,
    CheckpointCoordinatorConfig, CheckpointCoordinatorHandle, DiskStorage, NonceCounter,
    ProviderState, ReplicaSyncCoordinator, ReplicaSyncCoordinatorConfig,
    ReplicaSyncCoordinatorHandle, StateNonceCounter, StateProviderInfo, Storage, StorageBackend,
};
use clap::Parser;
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::{Arc, RwLock};
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

            state.nonce_counter = setup_nonce_counter(&cli, &state.provider_id).await;
            state.provider_info = setup_provider_info(&cli, &state.provider_id).await;

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
                "Checkpoint coordinator needs a chain client (--keyfile + reachable chain). Skipping."
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

/// Fetch the provider's on-chain registration info once and store it.
///
/// Returns `None` (and logs a warning) if anything goes wrong, so a transient
/// chain hiccup or an unregistered provider doesn't take the whole node down.
async fn setup_provider_info(cli: &Cli, provider_id: &str) -> StateProviderInfo {
    let provider_account = match sp_runtime::AccountId32::from_str(provider_id) {
        Ok(account) => account,
        Err(e) => {
            tracing::warn!("invalid provider SS58 {provider_id}: {e:?}");
            return None;
        }
    };

    let client = storage_client::ProviderClient::new(
        storage_client::ClientConfig {
            chain_ws_url: cli.rpc.chain_rpc.clone(),
            ..Default::default()
        },
        provider_id.to_string(),
    );
    let mut client = match client {
        Ok(client) => client,
        Err(e) => {
            tracing::warn!("failed to build provider client: {e:?}");
            return None;
        }
    };
    if let Err(e) = client.connect().await {
        tracing::warn!("failed to connect to chain: {e:?}");
        return None;
    }

    let info = match client.get_provider_info(&provider_account).await {
        Ok(Some(info)) => info,
        Ok(None) => {
            tracing::warn!(
                "provider {provider_id} is not registered on chain; \
                 register it before starting the node"
            );
            return None;
        }
        Err(e) => {
            tracing::warn!("failed to fetch provider info: {e:?}");
            return None;
        }
    };

    tracing::info!(
        "Loaded on-chain provider info: price_per_byte={}, duration=[{}, {}], max_capacity={}, accepting_primary={}",
        info.price_per_byte,
        info.min_duration,
        info.max_duration,
        info.max_capacity,
        info.accepting_primary,
    );

    Some(Arc::new(RwLock::new(info)))
}

/// Create the in-memory nonce counter and bootstrap it from the chain's
/// `ProviderReplayState.hsn`. The chain is the source of truth, so there
/// is nothing to persist locally.
async fn setup_nonce_counter(cli: &Cli, provider_id: &str) -> StateNonceCounter {
    // Bootstrap from on-chain hsn. Best-effort: if the chain isn't
    // reachable yet, set to None
    let provider_account = match sp_runtime::AccountId32::from_str(provider_id) {
        Ok(account) => account,
        Err(e) => {
            tracing::warn!("invalid provider SS58 {provider_id}: {e:?}");
            return None;
        }
    };
    match storage_client::ProviderClient::fetch_replay_hsn(&cli.rpc.chain_rpc, &provider_account)
        .await
    {
        Ok(Some(hsn)) => {
            tracing::info!(
                "Bootstrapping nonce counter from on-chain hsn {} for provider {}",
                hsn,
                provider_id,
            );
            let counter = NonceCounter::new(1);
            counter.bootstrap_from_hsn(hsn);
            Some(Arc::new(counter))
        }
        Ok(None) => {
            tracing::warn!("No on-chain replay state for provider {} yet.", provider_id,);
            None
        }
        Err(e) => {
            tracing::warn!("Failed to bootstrap nonce counter from chain: {}.", e,);
            None
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
