// SPDX-License-Identifier: GPL-3.0-only

//! CLI argument parsing for the storage provider node.

use clap::Parser;
use provider_chain::chain_connection::{ChainTransport, SpecSource};
use provider_storage::StorageBackendSpec;
use std::path::PathBuf;

/// Placeholder provider ID used when no identity is configured.
pub const DEFAULT_PROVIDER_ID: &str = "0x0000000000000000000000000000000000000000";

/// Storage backend to run.
///
/// `rename_all` keeps the values lower-case instead of clap's kebab-case
/// default, so an engine reads as `rocksdb` rather than `rocks-db`.
#[derive(Clone, Debug, clap::ValueEnum)]
#[value(rename_all = "lower")]
pub enum StorageBackendKind {
    /// Persistent RocksDB storage.
    RocksDb,
}

impl StorageBackendKind {
    /// The spec for this engine, rooted at `path`.
    ///
    /// The only place a kind becomes a spec, so a new engine is one arm here.
    pub fn spec(&self, path: PathBuf) -> StorageBackendSpec {
        match self {
            Self::RocksDb => StorageBackendSpec::RocksDb { path },
        }
    }
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
    pub replica_sync: ReplicaSyncParams,

    #[clap(flatten)]
    pub challenge_responder: ChallengeResponderParams,

    #[clap(flatten)]
    pub auth: AuthParams,
}

/// Parameters for the storage backend.
#[derive(Debug, clap::Args)]
pub struct StorageParams {
    /// Storage backend to use.
    #[arg(long, value_enum, default_value_t = StorageBackendKind::RocksDb)]
    pub storage_backend: StorageBackendKind,

    /// Directory holding the chunks, the MMR state and the nonce counter.
    #[arg(long, default_value = "./provider-data", env = "STORAGE_PATH")]
    pub storage_path: PathBuf,
}

impl StorageParams {
    /// The backend these flags describe.
    pub fn spec(&self) -> StorageBackendSpec {
        self.storage_backend.spec(self.storage_path.clone())
    }
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

    /// How to talk to the chain: an external RPC node or the embedded smoldot
    /// light client (which needs no operated RPC infrastructure).
    #[arg(
        long,
        value_enum,
        value_name = "TRANSPORT",
        default_value_t = TransportKind::Rpc,
        env = "CHAIN_TRANSPORT"
    )]
    pub chain_transport: TransportKind,

    /// Relay-chain spec for the light transport: a spec file path (a raw
    /// spec with reachable boot nodes — the trust-preserving option), or a
    /// ws:// / wss:// node URL to fetch it from at startup (dev convenience;
    /// trusts that node).
    #[arg(long, value_name = "FILE|WS_URL", env = "RELAY_CHAIN_SPEC")]
    pub relay_chain_spec: Option<String>,

    /// Parachain spec for the light transport: a spec file path (a raw spec
    /// with boot nodes serving the light request-response protocols), or a
    /// ws:// / wss:// node URL to fetch it from. Defaults to fetching from
    /// --chain-rpc (dev only).
    #[arg(long, value_name = "FILE|WS_URL", env = "PARA_CHAIN_SPEC")]
    pub para_chain_spec: Option<String>,

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

    /// Comma-separated list of browser origins allowed via CORS
    /// (e.g. "https://app.example.com,http://localhost:5174").
    /// When unset, all origins are allowed (permissive) — set this in production.
    #[arg(
        long,
        value_name = "ORIGIN",
        env = "CORS_ALLOWED_ORIGINS",
        value_delimiter = ','
    )]
    pub cors_allowed_origins: Option<Vec<String>>,
}

/// Chain transport selection for `--chain-transport`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum TransportKind {
    /// External RPC node over WebSocket (`--chain-rpc`).
    Rpc,
    /// Embedded smoldot light client. Chain specs come from
    /// `--relay-chain-spec` / `--para-chain-spec` — a spec file path, or a
    /// ws:// URL to fetch from (dev only); the parachain spec defaults to
    /// fetching from `--chain-rpc`.
    Light,
}

/// A spec argument is a node URL to fetch from when it looks like a
/// WebSocket URL, and a spec file path otherwise.
fn spec_source(value: &str) -> SpecSource {
    if value.starts_with("ws://") || value.starts_with("wss://") {
        SpecSource::FetchFromRpc(value.to_string())
    } else {
        SpecSource::File(PathBuf::from(value))
    }
}

impl RpcParams {
    /// Resolve the CLI flags into a concrete [`ChainTransport`].
    ///
    /// Errors when the light transport has no relay spec source.
    pub fn chain_transport(&self) -> Result<ChainTransport, String> {
        match self.chain_transport {
            TransportKind::Rpc => Ok(ChainTransport::Rpc {
                url: self.chain_rpc.clone(),
            }),
            TransportKind::Light => Ok(ChainTransport::LightClient {
                relay_spec: self.relay_chain_spec.as_deref().map(spec_source).ok_or(
                    "--chain-transport light needs --relay-chain-spec (a spec file, or, \
                     for dev, a ws:// node URL to fetch it from)"
                        .to_string(),
                )?,
                para_spec: self
                    .para_chain_spec
                    .as_deref()
                    .map(spec_source)
                    .unwrap_or_else(|| SpecSource::FetchFromRpc(self.chain_rpc.clone())),
            }),
        }
    }
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

/// Parameters for authentication and authorization.
#[derive(Debug, clap::Args)]
pub struct AuthParams {
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

/// Parameters for the challenge responder background service.
#[derive(Debug, clap::Args)]
pub struct ChallengeResponderParams {
    /// Enable the autonomous challenge responder. Without this flag, the
    /// provider relies on an external orchestrator (e.g. the client SDK
    /// driving challenges) to surface incoming challenges via HTTP proof
    /// endpoints. With this flag, the provider polls chain state itself.
    #[arg(long, env = "ENABLE_CHALLENGE_RESPONDER")]
    pub enable_challenge_responder: bool,

    /// Seconds between safety-net `Challenges` reconciliation scans.
    /// Challenges are normally handled event-driven from the finalized-block
    /// stream; this scan only catches events lost to edge cases. 0 disables it.
    #[arg(
        long,
        value_name = "SECONDS",
        default_value_t = 300,
        env = "CHALLENGE_POLL_INTERVAL"
    )]
    pub challenge_poll_interval: u64,
}

/// Parameters for replica synchronization.
#[derive(Debug, clap::Args)]
pub struct ReplicaSyncParams {
    /// Enable autonomous replica sync.
    #[arg(long, env = "ENABLE_REPLICA_SYNC")]
    pub enable_replica_sync: bool,

    /// Seconds between safety-net replica duty reconciliation passes.
    /// Duties are normally discovered event-driven from the finalized-block
    /// stream; this pass only catches events lost to edge cases. 0 disables it.
    #[arg(
        long,
        value_name = "SECONDS",
        default_value_t = 600,
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
        // Persistence is the default: an operator who passes no storage flags
        // must not silently get a backend that drops everything on restart.
        assert!(matches!(
            cli.storage.storage_backend,
            StorageBackendKind::RocksDb
        ));
        assert_eq!(cli.storage.storage_path, PathBuf::from("./provider-data"));
        assert_eq!(cli.rpc.bind_addr, "0.0.0.0:3333");
        assert_eq!(cli.rpc.chain_rpc, "ws://127.0.0.1:2222");
        assert!(cli.key.keyfile.is_none());
        assert!(cli.key.provider_id.is_none());
        assert!(!cli.replica_sync.enable_replica_sync);
        assert_eq!(cli.replica_sync.replica_poll_interval, 600);
        assert_eq!(cli.replica_sync.replica_sync_timeout, 300);
        assert_eq!(cli.replica_sync.replica_max_concurrent, 3);
        assert!(!cli.challenge_responder.enable_challenge_responder);
        assert_eq!(cli.challenge_responder.challenge_poll_interval, 300);
    }

    #[test]
    fn all_args_parse() {
        let cli = Cli::try_parse_from([
            "storage-provider-node",
            "--storage-backend",
            "rocksdb",
            "--storage-path",
            "/data",
            "--bind-addr",
            "127.0.0.1:4444",
            "--chain-rpc",
            "ws://example.com:9944",
            "--keyfile",
            "/tmp/test-key",
            "--enable-replica-sync",
            "--replica-poll-interval",
            "30",
            "--replica-sync-timeout",
            "600",
            "--replica-max-concurrent",
            "5",
        ])
        .unwrap();

        assert!(matches!(
            cli.storage.storage_backend,
            StorageBackendKind::RocksDb
        ));
        assert_eq!(cli.storage.storage_path, PathBuf::from("/data"));
        assert_eq!(cli.rpc.bind_addr, "127.0.0.1:4444");
        assert_eq!(cli.rpc.chain_rpc, "ws://example.com:9944");
        assert_eq!(
            cli.key.keyfile.as_ref().unwrap().to_str().unwrap(),
            "/tmp/test-key"
        );
        assert!(cli.replica_sync.enable_replica_sync);
        assert_eq!(cli.replica_sync.replica_poll_interval, 30);
        assert_eq!(cli.replica_sync.replica_sync_timeout, 600);
        assert_eq!(cli.replica_sync.replica_max_concurrent, 5);
    }

    /// Values are the engine names, and each maps to the matching spec.
    #[test]
    fn backend_value_is_the_engine_name() {
        let spec = |value: &str| {
            Cli::try_parse_from(["storage-provider-node", "--storage-backend", value])
                .map(|cli| cli.storage.spec())
        };

        assert_eq!(
            spec("rocksdb").unwrap(),
            StorageBackendSpec::RocksDb {
                path: "./provider-data".into()
            }
        );

        for rejected in ["disk", "rocks-db", "inmemory"] {
            assert!(
                spec(rejected).is_err(),
                "--storage-backend {rejected} should be rejected"
            );
        }
    }

    #[test]
    fn transport_defaults_to_rpc() {
        let cli = Cli::try_parse_from(["storage-provider-node"]).unwrap();
        assert!(matches!(cli.rpc.chain_transport, TransportKind::Rpc));
        let transport = cli.rpc.chain_transport().unwrap();
        assert!(matches!(transport, ChainTransport::Rpc { url } if url == "ws://127.0.0.1:2222"));
    }

    #[test]
    fn light_transport_resolves_spec_sources() {
        // Spec files win over RPC fetching.
        let cli = Cli::try_parse_from([
            "storage-provider-node",
            "--chain-transport",
            "light",
            "--relay-chain-spec",
            "/specs/relay.json",
            "--para-chain-spec",
            "/specs/para.json",
        ])
        .unwrap();
        let ChainTransport::LightClient {
            relay_spec,
            para_spec,
        } = cli.rpc.chain_transport().unwrap()
        else {
            panic!("expected light transport");
        };
        assert!(
            matches!(relay_spec, SpecSource::File(p) if p == PathBuf::from("/specs/relay.json").as_path())
        );
        assert!(
            matches!(para_spec, SpecSource::File(p) if p == PathBuf::from("/specs/para.json").as_path())
        );

        // A ws:// spec argument means "fetch from this node"; without a para
        // spec at all, the para spec fetches from --chain-rpc.
        let cli = Cli::try_parse_from([
            "storage-provider-node",
            "--chain-transport",
            "light",
            "--relay-chain-spec",
            "ws://127.0.0.1:9900",
        ])
        .unwrap();
        let ChainTransport::LightClient {
            relay_spec,
            para_spec,
        } = cli.rpc.chain_transport().unwrap()
        else {
            panic!("expected light transport");
        };
        assert!(
            matches!(relay_spec, SpecSource::FetchFromRpc(url) if url == "ws://127.0.0.1:9900")
        );
        assert!(matches!(para_spec, SpecSource::FetchFromRpc(url) if url == "ws://127.0.0.1:2222"));
    }

    #[test]
    fn light_transport_without_relay_source_errors() {
        let cli =
            Cli::try_parse_from(["storage-provider-node", "--chain-transport", "light"]).unwrap();
        let err = cli.rpc.chain_transport().unwrap_err();
        assert!(
            err.contains("--relay-chain-spec"),
            "unexpected error: {err}"
        );
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
