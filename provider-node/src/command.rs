// SPDX-License-Identifier: GPL-3.0-only

//! Node startup and runtime orchestration.

use crate::{
    chain_state_coordinator::ChainStateCoordinator,
    cli::{Cli, DEFAULT_PROVIDER_ID},
    create_router,
    membership::{BlockEventInvalidations, ChainMembershipResolver},
    subxt_client::SubxtChainClient,
    ChainStateCoordinatorHandle, ChallengeResponder, ChallengeResponderConfig,
    ChallengeResponderHandle, GcCoordinator, GcCoordinatorConfig, GcCoordinatorHandle,
    ProviderDeps, ProviderState, ReplicaSyncCoordinator, ReplicaSyncCoordinatorConfig,
    ReplicaSyncCoordinatorHandle,
};
use clap::Parser;
use provider_auth::Authenticator;
use provider_chain::{
    chain_connection::{self, ChainHandle, ChainTransport},
    BlockEvent, BlockEventRx, BlockEventTx, EVENT_CHANNEL_CAPACITY,
};
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, watch};
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

    // One chain connection for the whole node, published through a watch
    // channel. The chain-state coordinator owns the sender and rebuilds the
    // connection on loss or stall; every consumer (HTTP auth, the signing
    // client, coordinators) borrows the current handle from the receiver.
    let transport = cli.rpc.chain_transport()?;
    let (chain_tx, chain_rx) = watch::channel::<Option<ChainHandle>>(None);
    // Per-block event fan-out from the chain-state coordinator to the
    // background coordinators.
    let (events_tx, _) = broadcast::channel::<BlockEvent>(EVENT_CHANNEL_CAPACITY);

    // Connect eagerly so auth lookups and the one-shot multiaddr sync work
    // right away; failure is non-fatal (the coordinator retries in the
    // background and republishes).
    match chain_connection::connect(&transport).await {
        Ok(handle) => {
            chain_tx.send_replace(Some(handle));
        }
        Err(e) => tracing::warn!("Chain unreachable at startup ({e}); retrying in the background"),
    }

    let backend = cli.storage.spec();
    tracing::info!("Storage backend: {backend}");
    let (storage, nonce_store) = backend.build()?;

    // Membership-based auth over the chain's bucket member sets, resolved
    // through the shared watch connection. Subscribed here rather than after
    // the chain-state coordinator starts, so the cache cannot miss the
    // bootstrap `Resubscribed` the coordinator broadcasts on first connect.
    let resolver = ChainMembershipResolver::new(chain_rx.clone());
    // Incoherent, not unsafe - warn rather than clamp an explicit choice.
    if cli.auth.auth_max_stale <= cli.auth.auth_cache_ttl {
        tracing::warn!(
            "--auth-max-stale ({}s) is below --auth-cache-ttl ({}s): a cached member set \
             will never be served once the chain is unreachable",
            cli.auth.auth_max_stale,
            cli.auth.auth_cache_ttl
        );
    }
    let auth = Arc::new(
        Authenticator::new(resolver)
            .with_ttl(Duration::from_secs(cli.auth.auth_cache_ttl))
            .with_max_skew(Duration::from_secs(cli.auth.auth_max_skew))
            .with_max_stale(Duration::from_secs(cli.auth.auth_max_stale))
            .with_max_entries(cli.auth.auth_cache_max_entries)
            .with_invalidations(BlockEventInvalidations::new(events_tx.subscribe())),
    );
    tracing::info!(
        "Auth: membership cache_ttl={}s, max_stale={}s, max_skew={}s, max_entries={}",
        cli.auth.auth_cache_ttl,
        cli.auth.auth_max_stale,
        cli.auth.auth_max_skew,
        cli.auth.auth_cache_max_entries
    );

    let deps = ProviderDeps {
        storage,
        nonce_store,
        auth,
    };

    // Resolve provider identity
    let seed = cli.key.load_seed()?;

    // The signing chain client shared by every coordinator: one signer (the
    // provider's own account) over the shared watch connection; coordinators
    // each get a cheap clone. Requires a signing key, so this is only
    // available when a seed was provided.
    let chain_client = match &seed {
        Some(seed) => match SubxtChainClient::new(chain_rx.clone(), seed) {
            Ok(client) => Some(client),
            Err(e) => {
                tracing::error!("Failed to create chain client: {}", e);
                None
            }
        },
        None => None,
    };

    let state = Arc::new(
        match &seed {
            Some(seed) => {
                let state = ProviderState::with_seed(deps, seed)?;
                tracing::info!("Signing enabled for account: {}", state.provider_id);
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

                ProviderState::with_provider_id(deps, provider_id)
            }
        }
        .with_cors_origins(cli.rpc.cors_allowed_origins.clone())
        // The /delete frozen check reads chain state through the same
        // client; nodes without one skip the check.
        .with_gc_chain(
            chain_client
                .as_ref()
                .map(|c| Arc::new(c.clone()) as Arc<dyn crate::gc_coordinator::GcChainClient>),
        ),
    );

    // Subscribe the coordinators before the follower starts, so none of them
    // can miss the initial `Resubscribed` bootstrap event.
    let replica_events = events_tx.subscribe();
    let challenge_events = events_tx.subscribe();
    let gc_events = events_tx.subscribe();

    // Start optional background services (failures are non-fatal)
    let _chain_state_handle =
        start_chain_state_coordinator(transport, chain_tx, events_tx, state.clone());
    let _replica_sync_handle =
        start_replica_sync_coordinator(&cli, chain_client.as_ref(), replica_events, state.clone())
            .await;
    let _challenge_responder_handle =
        start_challenge_responder(&cli, chain_client.as_ref(), challenge_events, state.clone())
            .await;
    let _gc_handle = start_gc_coordinator(chain_client.as_ref(), gc_events, state.clone());

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

/// Start the chain-state coordinator, which keeps
/// `chain_state.current_anchor_block` and `chain_state.provider_info` in sync
/// with the chain, and broadcasts bucket-membership changes on the
/// block-event fan-out, which the auth membership cache drains.
///
/// Returns `None` only when the provider id isn't a valid account. The
/// coordinator itself never fails to start: it connects in the background and
/// retries with a backoff if the chain is unreachable, so `current_anchor_block`
/// is populated as soon as the chain comes up.
fn start_chain_state_coordinator(
    transport: ChainTransport,
    chain_tx: watch::Sender<Option<ChainHandle>>,
    events_tx: BlockEventTx,
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
        transport,
        provider_account,
        state.chain_state.clone(),
        chain_tx,
        events_tx,
    );

    tracing::info!("Chain-state coordinator started (retries until the chain is reachable)");
    Some(coordinator.start())
}

/// Start the GC coordinator, which physically erases pruned/torn-down data
/// once on-chain liability has passed and keeps bucket quotas synced from
/// agreements. Chainless nodes (no signing key/connection) run without it:
/// nothing can be checkpointed or challenged, so stashes simply persist.
fn start_gc_coordinator(
    chain_client: Option<&SubxtChainClient>,
    events_rx: BlockEventRx,
    state: Arc<ProviderState>,
) -> Option<GcCoordinatorHandle> {
    let chain_client = chain_client?.clone();

    let coordinator = GcCoordinator::new(
        GcCoordinatorConfig::default(),
        state,
        Arc::new(chain_client),
    );
    tracing::info!("GC coordinator started");
    Some(coordinator.start(events_rx))
}

async fn start_replica_sync_coordinator(
    cli: &Cli,
    chain_client: Option<&SubxtChainClient>,
    events_rx: BlockEventRx,
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

    match coordinator.start(events_rx, None).await {
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

async fn start_challenge_responder(
    cli: &Cli,
    chain_client: Option<&SubxtChainClient>,
    events_rx: BlockEventRx,
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

    let provider_account = match sp_runtime::AccountId32::from_str(&state.provider_id) {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!(
                "challenge responder: invalid provider SS58 '{}': {e:?}",
                state.provider_id
            );
            return None;
        }
    };

    let config = ChallengeResponderConfig {
        poll_interval: Duration::from_secs(cli.challenge_responder.challenge_poll_interval),
        ..ChallengeResponderConfig::new(provider_account)
    };

    let responder = ChallengeResponder::new(
        config,
        state.challenge_proof_source(),
        Box::new(chain_client),
    );

    match responder.start(events_rx, None).await {
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
