//! CLI argument parsing for the storage provider node.
//!
//! Follows polkadot-sdk style: parameter groups as separate structs composed
//! via `#[clap(flatten)]`.

use crate::{
    create_router, CheckpointCoordinator, CheckpointCoordinatorConfig, DiskStorage, ProviderState,
    ReplicaSyncCoordinator, ReplicaSyncCoordinatorConfig, Storage, StorageBackend,
};
use clap::Parser;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

/// Storage backend mode.
#[derive(Clone, Debug, clap::ValueEnum)]
pub enum StorageMode {
    /// In-memory storage (data lost on restart).
    Inmemory,
    /// Persistent disk storage.
    Disk,
}

/// Storage Provider Node - Off-chain storage server for Web3 Storage.
#[derive(Debug, Parser)]
#[command(name = "storage-provider-node", version, about)]
pub struct Cli {
    #[clap(flatten)]
    pub storage: StorageParams,

    #[clap(flatten)]
    pub rpc: RpcParams,

    #[clap(flatten)]
    pub key: KeyParams,

    #[clap(flatten)]
    pub checkpoint: CheckpointParams,

    #[clap(flatten)]
    pub replica_sync: ReplicaSyncParams,
}

/// Parameters for the storage backend.
#[derive(Debug, clap::Args)]
pub struct StorageParams {
    /// Storage backend to use.
    #[arg(long, value_enum, default_value_t = StorageMode::Inmemory)]
    pub storage_mode: StorageMode,

    /// Path for persistent data (only used with --storage-mode disk).
    #[arg(long, default_value = "./provider-data", env = "STORAGE_PATH")]
    pub storage_path: String,
}

/// Parameters for network endpoints.
#[derive(Debug, clap::Args)]
pub struct RpcParams {
    /// Address to bind the HTTP server to.
    #[arg(
        long,
        value_name = "ADDR",
        default_value = "0.0.0.0:3333",
        env = "BIND_ADDR"
    )]
    pub bind_addr: String,

    /// WebSocket URL for the parachain RPC.
    #[arg(
        long,
        value_name = "URL",
        default_value = "ws://127.0.0.1:2222",
        env = "CHAIN_RPC"
    )]
    pub chain_rpc: String,
}

/// Parameters for provider identity and signing keys.
#[derive(Debug, clap::Args)]
pub struct KeyParams {
    /// Path to a file containing the secret seed phrase or derivation path
    /// (e.g., "//Alice"). The file must not be group- or world-readable
    /// (permissions <= 0600 on Unix).
    #[arg(long, value_name = "FILE")]
    pub keyfile: Option<PathBuf>,

    /// Provider account ID (SS58). Used when --keyfile is not set; disables
    /// signing capability.
    #[arg(long, value_name = "ACCOUNT", env = "PROVIDER_ID")]
    pub provider_id: Option<String>,

    /// Use a development key (equivalent to --keyfile containing "//Alice").
    #[arg(long, conflicts_with = "keyfile")]
    pub dev: bool,
}

impl KeyParams {
    /// Resolve the signing seed from CLI parameters.
    ///
    /// Priority:
    /// 1. `--dev` returns `"//Alice"`
    /// 2. `--keyfile` reads the file contents
    /// 3. Neither returns `None` (provider-id mode without signing)
    pub fn load_seed(&self) -> Result<Option<String>, String> {
        if self.dev {
            return Ok(Some("//Alice".to_string()));
        }

        let Some(ref path) = self.keyfile else {
            return Ok(None);
        };

        if !path.exists() {
            return Err(format!("Keyfile not found: {}", path.display()));
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let metadata = std::fs::metadata(path)
                .map_err(|e| format!("Cannot read keyfile metadata: {e}"))?;
            let mode = metadata.permissions().mode();
            if mode & 0o077 != 0 {
                return Err(format!(
                    "Keyfile {} has insecure permissions {:o}. Run: chmod 600 {}",
                    path.display(),
                    mode & 0o777,
                    path.display(),
                ));
            }
        }

        let contents = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read keyfile {}: {e}", path.display()))?;
        let seed = contents.trim().to_string();

        if seed.is_empty() {
            return Err(format!("Keyfile {} is empty", path.display()));
        }

        Ok(Some(seed))
    }
}

/// Parameters for the checkpoint coordinator.
#[derive(Debug, clap::Args)]
pub struct CheckpointParams {
    /// Enable the background checkpoint coordinator.
    #[arg(long, env = "ENABLE_CHECKPOINT_COORDINATOR")]
    pub enable_checkpoint_coordinator: bool,
}

/// Parameters for replica synchronization.
#[derive(Debug, clap::Args)]
pub struct ReplicaSyncParams {
    /// Enable autonomous replica sync.
    #[arg(long, env = "ENABLE_REPLICA_SYNC")]
    pub enable_replica_sync: bool,

    /// Seconds between replica sync poll checks.
    #[arg(
        long,
        value_name = "SECS",
        default_value_t = 12,
        env = "REPLICA_POLL_INTERVAL"
    )]
    pub replica_poll_interval: u64,

    /// Seconds before a replica sync operation times out.
    #[arg(
        long,
        value_name = "SECS",
        default_value_t = 300,
        env = "REPLICA_SYNC_TIMEOUT"
    )]
    pub replica_sync_timeout: u64,

    /// Maximum number of concurrent bucket syncs.
    #[arg(
        long,
        value_name = "COUNT",
        default_value_t = 3,
        env = "REPLICA_MAX_CONCURRENT"
    )]
    pub replica_max_concurrent: usize,
}

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
                cli.storage.storage_path
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
                .unwrap_or_else(|| "0x0000000000000000000000000000000000000000".to_string());
            tracing::warn!(
                "No --keyfile or --dev set, using --provider-id without signing: {}",
                provider_id
            );
            Arc::new(ProviderState::new(storage, provider_id))
        }
    };

    let _checkpoint_handle = start_checkpoint_coordinator(&cli, state.clone()).await;
    let _replica_sync_handle = start_replica_sync_coordinator(&cli, state.clone()).await;

    tracing::info!("Starting storage provider node on {}", cli.rpc.bind_addr);

    let listener = tokio::net::TcpListener::bind(&cli.rpc.bind_addr).await?;
    let app = create_router(state.clone());
    axum::serve(listener, app).await?;

    Ok(())
}

async fn start_checkpoint_coordinator(
    cli: &Cli,
    state: Arc<ProviderState>,
) -> Option<crate::CheckpointCoordinatorHandle> {
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
) -> Option<crate::ReplicaSyncCoordinatorHandle> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_values() {
        let cli = Cli::try_parse_from(["storage-provider-node"]).unwrap();
        assert!(matches!(cli.storage.storage_mode, StorageMode::Inmemory));
        assert_eq!(cli.storage.storage_path, "./provider-data");
        assert_eq!(cli.rpc.bind_addr, "0.0.0.0:3333");
        assert_eq!(cli.rpc.chain_rpc, "ws://127.0.0.1:2222");
        assert!(!cli.key.dev);
        assert!(cli.key.keyfile.is_none());
        assert!(cli.key.provider_id.is_none());
        assert!(!cli.checkpoint.enable_checkpoint_coordinator);
        assert!(!cli.replica_sync.enable_replica_sync);
        assert_eq!(cli.replica_sync.replica_poll_interval, 12);
        assert_eq!(cli.replica_sync.replica_sync_timeout, 300);
        assert_eq!(cli.replica_sync.replica_max_concurrent, 3);
    }

    #[test]
    fn all_args_parse() {
        let cli = Cli::try_parse_from([
            "storage-provider-node",
            "--storage-mode",
            "disk",
            "--storage-path",
            "/data",
            "--bind-addr",
            "127.0.0.1:4444",
            "--chain-rpc",
            "ws://example.com:9944",
            "--keyfile",
            "/tmp/test-key",
            "--enable-checkpoint-coordinator",
            "--enable-replica-sync",
            "--replica-poll-interval",
            "30",
            "--replica-sync-timeout",
            "600",
            "--replica-max-concurrent",
            "5",
        ])
        .unwrap();

        assert!(matches!(cli.storage.storage_mode, StorageMode::Disk));
        assert_eq!(cli.storage.storage_path, "/data");
        assert_eq!(cli.rpc.bind_addr, "127.0.0.1:4444");
        assert_eq!(cli.rpc.chain_rpc, "ws://example.com:9944");
        assert_eq!(
            cli.key.keyfile.as_ref().unwrap().to_str().unwrap(),
            "/tmp/test-key"
        );
        assert!(cli.checkpoint.enable_checkpoint_coordinator);
        assert!(cli.replica_sync.enable_replica_sync);
        assert_eq!(cli.replica_sync.replica_poll_interval, 30);
        assert_eq!(cli.replica_sync.replica_sync_timeout, 600);
        assert_eq!(cli.replica_sync.replica_max_concurrent, 5);
    }

    #[test]
    fn dev_flag() {
        let cli = Cli::try_parse_from(["storage-provider-node", "--dev"]).unwrap();
        assert!(cli.key.dev);
        assert_eq!(cli.key.load_seed().unwrap(), Some("//Alice".to_string()));
    }

    #[test]
    fn dev_conflicts_with_keyfile() {
        let result =
            Cli::try_parse_from(["storage-provider-node", "--dev", "--keyfile", "/tmp/key"]);
        assert!(result.is_err());
    }

    #[test]
    fn load_seed_missing_file() {
        let params = KeyParams {
            keyfile: Some(PathBuf::from("/nonexistent/path/to/keyfile")),
            provider_id: None,
            dev: false,
        };
        let err = params.load_seed().unwrap_err();
        assert!(err.contains("not found"), "unexpected error: {err}");
    }

    #[test]
    fn load_seed_empty_file() {
        let dir = std::env::temp_dir().join("provider-cli-test-empty");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("empty-keyfile");
        std::fs::write(&path, "  \n").unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }

        let params = KeyParams {
            keyfile: Some(path.clone()),
            provider_id: None,
            dev: false,
        };
        let err = params.load_seed().unwrap_err();
        assert!(err.contains("empty"), "unexpected error: {err}");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_seed_from_keyfile() {
        let dir = std::env::temp_dir().join("provider-cli-test-read");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test-keyfile");
        std::fs::write(&path, "//Charlie\n").unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }

        let params = KeyParams {
            keyfile: Some(path.clone()),
            provider_id: None,
            dev: false,
        };
        assert_eq!(params.load_seed().unwrap(), Some("//Charlie".to_string()));

        let _ = std::fs::remove_file(&path);
    }

    #[cfg(unix)]
    #[test]
    fn load_seed_rejects_world_readable() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join("provider-cli-test-perms");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("insecure-keyfile");
        std::fs::write(&path, "//Alice").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        let params = KeyParams {
            keyfile: Some(path.clone()),
            provider_id: None,
            dev: false,
        };
        let err = params.load_seed().unwrap_err();
        assert!(
            err.contains("insecure permissions"),
            "unexpected error: {err}"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_seed_none_without_keyfile_or_dev() {
        let params = KeyParams {
            keyfile: None,
            provider_id: None,
            dev: false,
        };
        assert_eq!(params.load_seed().unwrap(), None);
    }
}
