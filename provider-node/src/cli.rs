// SPDX-License-Identifier: GPL-3.0-only

//! CLI argument parsing for the storage provider node.

use clap::Parser;
use std::path::PathBuf;

/// Placeholder provider ID used when no identity is configured.
pub const DEFAULT_PROVIDER_ID: &str = "0x0000000000000000000000000000000000000000";

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

    #[clap(flatten)]
    pub auth: AuthParams,
}

/// Parameters for the storage backend.
#[derive(Debug, clap::Args)]
pub struct StorageParams {
    /// Storage backend to use.
    #[arg(long, value_enum, default_value_t = StorageMode::Inmemory)]
    pub storage_mode: StorageMode,

    /// Path for persistent data (only used with --storage-mode disk).
    #[arg(long, default_value = "./provider-data", env = "STORAGE_PATH")]
    pub storage_path: PathBuf,
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

    /// Public multiaddr to advertise on chain instead of the bind-derived one.
    ///
    /// On hosted deployments the bind address (e.g. `0.0.0.0:3333`) is not
    /// reachable by clients, so the multiaddr sync would otherwise pin a
    /// useless `/ip4/127.0.0.1/tcp/3333` on chain. Set this to the
    /// externally-reachable address — typically a TLS-terminating reverse
    /// proxy — and the sync maintains it instead, e.g.
    /// `/dns4/example.com/tcp/443/tls/http/http-path/web3-storage-provider`.
    #[arg(long, value_name = "MULTIADDR", env = "PUBLIC_MULTIADDR")]
    pub public_multiaddr: Option<String>,
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
}

impl KeyParams {
    /// Resolve the signing seed from CLI parameters.
    ///
    /// Priority:
    /// 1. `--keyfile` reads the file contents (with permission checks)
    /// 2. No keyfile returns `None` (provider-id mode without signing)
    pub fn load_seed(&self) -> Result<Option<String>, String> {
        let Some(ref path) = self.keyfile else {
            return Ok(None);
        };

        read_secret_file(path).map(Some)
    }
}

/// Read a secret from a file, rejecting insecure permissions on Unix.
///
/// Opens the file, checks permissions on the open handle (Unix), and then
/// reads and trims the contents, and rejects empty files.
fn read_secret_file(path: &std::path::Path) -> Result<String, String> {
    use std::io::Read;

    let file = std::fs::File::open(path)
        .map_err(|e| format!("Failed to open keyfile {}: {e}", path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = file
            .metadata()
            .map_err(|e| format!("Cannot read keyfile metadata: {e}"))?
            .permissions()
            .mode();
        if mode & 0o077 != 0 {
            return Err(format!(
                "Keyfile {} has insecure permissions {:o}. Run: chmod 600 {}",
                path.display(),
                mode & 0o777,
                path.display(),
            ));
        }
    }

    let mut contents = String::new();
    std::io::BufReader::new(file)
        .read_to_string(&mut contents)
        .map_err(|e| format!("Failed to read keyfile {}: {e}", path.display()))?;

    let seed = contents.trim().to_string();
    if seed.is_empty() {
        return Err(format!("Keyfile {} is empty", path.display()));
    }

    Ok(seed)
}

/// Parameters for the checkpoint coordinator.
#[derive(Debug, clap::Args)]
pub struct CheckpointParams {
    /// Enable the background checkpoint coordinator.
    #[arg(long, env = "ENABLE_CHECKPOINT_COORDINATOR")]
    pub enable_checkpoint_coordinator: bool,
}

/// Parameters for authentication and authorization.
#[derive(Debug, clap::Args)]
pub struct AuthParams {
    /// Enable authentication and role-based access control.
    /// When disabled (default), all requests are allowed without auth headers.
    #[arg(long, env = "ENABLE_AUTH")]
    pub enable_auth: bool,

    /// Cache TTL in seconds for membership lookups from the chain.
    #[arg(
        long,
        value_name = "SECONDS",
        default_value_t = 30,
        env = "AUTH_CACHE_TTL"
    )]
    pub auth_cache_ttl: u64,

    /// Maximum allowed clock skew in seconds for request timestamps.
    #[arg(
        long,
        value_name = "SECONDS",
        default_value_t = 300,
        env = "AUTH_MAX_SKEW"
    )]
    pub auth_max_skew: u64,
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
        value_name = "SECONDS",
        default_value_t = 12,
        env = "REPLICA_POLL_INTERVAL"
    )]
    pub replica_poll_interval: u64,

    /// Seconds before a replica sync operation times out.
    #[arg(
        long,
        value_name = "SECONDS",
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_values() {
        let cli = Cli::try_parse_from(["storage-provider-node"]).unwrap();
        assert!(matches!(cli.storage.storage_mode, StorageMode::Inmemory));
        assert_eq!(cli.storage.storage_path, PathBuf::from("./provider-data"));
        assert_eq!(cli.rpc.bind_addr, "0.0.0.0:3333");
        assert_eq!(cli.rpc.chain_rpc, "ws://127.0.0.1:2222");
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
        assert_eq!(cli.storage.storage_path, PathBuf::from("/data"));
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
    fn load_seed_missing_file() {
        let params = KeyParams {
            keyfile: Some(PathBuf::from("/nonexistent/path/to/keyfile")),
            provider_id: None,
        };
        let err = params.load_seed().unwrap_err();
        assert!(err.contains("Failed to open"), "unexpected error: {err}");
    }

    #[test]
    fn load_seed_empty_file() {
        let path = std::env::temp_dir().join("cli-test-empty-keyfile");
        std::fs::write(&path, "  \n").unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }

        let params = KeyParams {
            keyfile: Some(path),
            provider_id: None,
        };
        let err = params.load_seed().unwrap_err();
        assert!(err.contains("empty"), "unexpected error: {err}");
    }

    #[test]
    fn load_seed_from_keyfile() {
        let path = std::env::temp_dir().join("cli-test-keyfile");
        std::fs::write(&path, "//Charlie\n").unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }

        let params = KeyParams {
            keyfile: Some(path),
            provider_id: None,
        };
        assert_eq!(params.load_seed().unwrap(), Some("//Charlie".to_string()));
    }

    #[cfg(unix)]
    #[test]
    fn load_seed_rejects_world_readable() {
        use std::os::unix::fs::PermissionsExt;

        let path = std::env::temp_dir().join("cli-test-insecure-keyfile");
        std::fs::write(&path, "//Alice").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        let params = KeyParams {
            keyfile: Some(path),
            provider_id: None,
        };
        let err = params.load_seed().unwrap_err();
        assert!(
            err.contains("insecure permissions"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn load_seed_none_without_keyfile() {
        let params = KeyParams {
            keyfile: None,
            provider_id: None,
        };
        assert_eq!(params.load_seed().unwrap(), None);
    }
}
