//! Node startup and runtime orchestration.

use crate::{
    auth::{ChainMembershipResolver, MembershipCache},
    cli::{Cli, StorageMode, DEFAULT_PROVIDER_ID},
    create_router, CheckpointCoordinator, CheckpointCoordinatorConfig, CheckpointCoordinatorHandle,
    DiskStorage, NonceCounter, ProviderState, ReplicaSyncCoordinator, ReplicaSyncCoordinatorConfig,
    ReplicaSyncCoordinatorHandle, Storage, StorageBackend,
};
use clap::Parser;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use subxt::dynamic::At;
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

            state.nonce_counter = Some(setup_nonce_counter(&cli, &state.provider_id).await?);

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

    // Start optional background services (failures are non-fatal)
    let checkpoint_handle = start_checkpoint_coordinator(&cli, &seed, state.clone()).await;
    if let Some(ref handle) = checkpoint_handle {
        state.set_checkpoint_handle(handle);
    }
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
    seed: &Option<String>,
    state: Arc<ProviderState>,
) -> Option<CheckpointCoordinatorHandle> {
    if !cli.checkpoint.enable_checkpoint_coordinator {
        return None;
    }

    let seed = match seed {
        Some(s) => s.clone(),
        None => {
            tracing::error!("Checkpoint coordinator requires --keyfile for signing. Skipping.");
            return None;
        }
    };

    let config = CheckpointCoordinatorConfig {
        chain_ws_url: cli.rpc.chain_rpc.clone(),
        seed: Some(seed),
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

/// Open the persistent nonce counter and bootstrap it from the chain's
/// `ProviderReplayState.hwm`. Default fallback to local storage, starting from 0
async fn setup_nonce_counter(
    cli: &Cli,
    provider_id: &str,
) -> Result<Arc<NonceCounter>, Box<dyn std::error::Error>> {
    let counter = match cli.storage.storage_mode {
        StorageMode::Inmemory => NonceCounter::in_memory(0),
        StorageMode::Disk => {
            let nonce_path = cli.storage.storage_path.join("nonce");
            if let Some(parent) = nonce_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    format!(
                        "failed to create nonce-counter parent dir {:?}: {}",
                        parent, e,
                    )
                })?;
            }
            NonceCounter::open(nonce_path.clone())
                .map_err(|e| format!("failed to open nonce counter at {:?}: {}", nonce_path, e))?
        }
    };

    // Bootstrap from on-chain hwm. Best-effort: if the chain isn't
    // reachable yet, fall back to the local counter — the on-chain
    // replay window will reject any out-of-range reissues anyway.
    let provider_account = sp_runtime::AccountId32::from_str(provider_id)
        .map_err(|e| format!("invalid provider SS58: {e:?}"))?;
    match storage_client::ProviderClient::fetch_replay_hwm(&cli.rpc.chain_rpc, &provider_account)
        .await
    {
        Ok(Some(hwm)) => {
            tracing::info!(
                "Bootstrapping nonce counter from on-chain hwm {} for provider {}",
                hwm,
                provider_id,
            );
            counter.bootstrap_from_hwm(hwm);
        }
        Ok(None) => {
            tracing::info!(
                "No on-chain replay state for provider {} yet; starting nonce counter from local storage",
                provider_id,
            );
        }
        Err(e) => {
            tracing::warn!(
                "Failed to bootstrap nonce counter from chain: {}; falling back to local storage",
                e,
            );
        }
    }

    Ok(Arc::new(counter))
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
    format!("/ip4/{host}/tcp/{port}")
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

    // Extract multiaddr from the encoded provider storage entry.
    // Decode to scale_value and extract the "multiaddr" field bytes.
    let current = {
        let decoded = match provider_value.to_value() {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("Could not decode provider value: {}, skipping sync", e);
                return;
            }
        };

        // Use At trait to navigate the decoded value
        let multiaddr_val = match decoded.at("multiaddr") {
            Some(v) => v,
            None => {
                tracing::warn!("No multiaddr field in provider info, skipping sync");
                return;
            }
        };

        // Extract bytes from the value — could be a sequence of u8 values
        fn extract_bytes<T>(val: &subxt::ext::scale_value::Value<T>) -> Vec<u8> {
            use subxt::ext::scale_value::{Composite, Primitive, ValueDef};
            match &val.value {
                // Sequence/composite of individual byte values
                ValueDef::Composite(Composite::Unnamed(items)) => items
                    .iter()
                    .filter_map(|item| match &item.value {
                        ValueDef::Primitive(Primitive::U128(n)) => Some(*n as u8),
                        _ => None,
                    })
                    .collect(),
                // Some subxt versions encode Vec<u8> as a Primitive::U256 or similar
                ValueDef::Primitive(Primitive::U128(n)) => {
                    // Single byte
                    vec![*n as u8]
                }
                _ => vec![],
            }
        }

        let bytes = extract_bytes(multiaddr_val);
        if bytes.is_empty() {
            // Debug: log the actual value structure to understand the encoding
            tracing::warn!(
                "multiaddr decoded as empty, value structure: {:?}",
                multiaddr_val
            );
        }
        String::from_utf8_lossy(&bytes).to_string()
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
    let signer = subxt_signer::sr25519::Keypair::from_uri(&uri).expect("valid keypair from seed");

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
