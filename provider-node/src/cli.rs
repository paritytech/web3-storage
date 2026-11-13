// SPDX-License-Identifier: GPL-3.0-only

//! CLI argument parsing for the storage provider node.

use crate::chain_connection::{ChainTransport, SpecSource};
use clap::Parser;
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
    /// Without a subcommand, the node starts.
    #[command(subcommand)]
    pub command: Option<Command>,

    #[clap(flatten)]
    pub storage: StorageParams,

    /// Global, so the flags are accepted before and after a subcommand.
    #[clap(flatten)]
    pub chain: ChainParams,

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

    /// Directory holding the chunks and the MMR state.
    #[arg(long, default_value = "./provider-data", env = "STORAGE_PATH")]
    pub storage_path: PathBuf,
}

impl StorageParams {
    /// The backend these flags describe.
    pub fn spec(&self) -> StorageBackendSpec {
        self.storage_backend.spec(self.storage_path.clone())
    }
}

/// Subcommands that run instead of the node.
#[derive(Debug, clap::Subcommand)]
pub enum Command {
    /// Manage this provider's on-chain registration.
    Helper(HelperCommand),
}

/// `storage-provider-node helper`: one signed provider management
/// transaction, then exit.
#[derive(Debug, clap::Args)]
pub struct HelperCommand {
    #[clap(flatten)]
    pub key: HelperKeyParams,

    #[command(subcommand)]
    pub action: HelperAction,
}

/// The provider management operations of `storage-provider-node helper`.
#[derive(Debug, clap::Subcommand)]
pub enum HelperAction {
    /// Create a new secret in `--keyfile` and print, as JSON, the account
    /// it derives. Fails if the file exists. Reads the chain's SS58 prefix
    /// from `--chain-rpc` for the address; when the chain is unreachable
    /// the generic prefix 42 is used and reported in the output.
    GenerateKey {
        /// Mnemonic length in words.
        #[arg(
            long,
            value_name = "COUNT",
            default_value_t = provider_helpers::DEFAULT_MNEMONIC_WORDS,
            value_parser = mnemonic_word_count
        )]
        words: usize,
    },

    /// Register the keyfile's account as a provider.
    Register {
        /// Network address clients connect to, e.g.
        /// `/ip4/1.2.3.4/tcp/3333` or
        /// `/dns4/example.com/tcp/443/tls/http/http-path/web3-storage-provider`.
        #[arg(long, value_name = "MULTIADDR", env = "PUBLIC_MULTIADDR")]
        multiaddr: String,

        /// Stake to lock, in the chain's smallest unit. Must be at least the
        /// chain's minimum provider stake.
        #[arg(long, value_name = "AMOUNT")]
        stake: u128,
    },

    /// Change the provider's settings. Only the given flags change; every
    /// other setting keeps its on-chain value.
    UpdateSettings(SettingsUpdateArgs),

    /// Announce deregistration. The chain releases the stake once
    /// `complete_deregister` is called after the announcement period.
    Deregister,
}

/// `--words` of `helper generate-key`: one of the BIP39 word counts.
fn mnemonic_word_count(value: &str) -> Result<usize, String> {
    let counts = provider_helpers::MNEMONIC_WORD_COUNTS;
    match value.parse::<usize>() {
        Ok(words) if counts.contains(&words) => Ok(words),
        _ => Err(format!(
            "must be one of {}",
            counts.map(|c| c.to_string()).join(", ")
        )),
    }
}

/// Flags of `helper update-settings`; each one is optional.
#[derive(Debug, Default, clap::Args)]
pub struct SettingsUpdateArgs {
    /// Shortest agreement duration, in anchor blocks, to accept. The chain
    /// counts blocks in `u32`.
    #[arg(long, value_name = "BLOCKS")]
    pub min_duration: Option<u32>,

    /// Longest agreement duration, in anchor blocks, to accept. The chain
    /// counts blocks in `u32`.
    #[arg(long, value_name = "BLOCKS")]
    pub max_duration: Option<u32>,

    /// Price per byte per anchor block, in the chain's smallest unit.
    #[arg(long, value_name = "AMOUNT")]
    pub price_per_byte: Option<u128>,

    /// Whether to accept new primary agreements.
    #[arg(long, value_name = "BOOL")]
    pub accepting_primary: Option<bool>,

    /// Price per confirmed replica sync, in the chain's smallest unit.
    /// Setting it makes the provider accept replica agreements.
    #[arg(long, value_name = "AMOUNT", conflicts_with = "no_replicas")]
    pub replica_sync_price: Option<u128>,

    /// Stop accepting replica agreements.
    #[arg(long)]
    pub no_replicas: bool,

    /// Whether to accept extensions of existing agreements.
    #[arg(long, value_name = "BOOL")]
    pub accepting_extensions: Option<bool>,

    /// Capacity limit in bytes; 0 means unlimited.
    #[arg(long, value_name = "BYTES")]
    pub max_capacity: Option<u64>,

    /// Smallest agreement size in bytes to accept; 0 means no minimum.
    #[arg(long, value_name = "BYTES")]
    pub min_agreement_bytes: Option<u64>,
}

impl SettingsUpdateArgs {
    /// The partial update these flags describe.
    pub fn update(&self) -> provider_helpers::ProviderSettingsUpdate {
        let replica_sync_price = if self.no_replicas {
            Some(None)
        } else {
            self.replica_sync_price.map(Some)
        };
        provider_helpers::ProviderSettingsUpdate {
            min_duration: self.min_duration.map(u64::from),
            max_duration: self.max_duration.map(u64::from),
            price_per_byte: self.price_per_byte,
            accepting_primary: self.accepting_primary,
            replica_sync_price,
            accepting_extensions: self.accepting_extensions,
            max_capacity: self.max_capacity,
            min_agreement_bytes: self.min_agreement_bytes,
        }
    }
}

/// Parameters for the HTTP server.
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

/// Parameters for the chain connection.
#[derive(Debug, clap::Args)]
pub struct ChainParams {
    /// WebSocket URL for the parachain RPC.
    #[arg(
        long,
        global = true,
        value_name = "URL",
        default_value = "ws://127.0.0.1:2222",
        env = "CHAIN_RPC"
    )]
    pub chain_rpc: String,

    /// How to talk to the chain: an external RPC node or the embedded smoldot
    /// light client (which needs no operated RPC infrastructure).
    #[arg(
        long,
        global = true,
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
    #[arg(
        long,
        global = true,
        value_name = "FILE|WS_URL",
        env = "RELAY_CHAIN_SPEC"
    )]
    pub relay_chain_spec: Option<String>,

    /// Parachain spec for the light transport: a spec file path (a raw spec
    /// with boot nodes serving the light request-response protocols), or a
    /// ws:// / wss:// node URL to fetch it from. Defaults to fetching from
    /// --chain-rpc (dev only).
    #[arg(
        long,
        global = true,
        value_name = "FILE|WS_URL",
        env = "PARA_CHAIN_SPEC"
    )]
    pub para_chain_spec: Option<String>,
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

impl ChainParams {
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
    /// Path to the keyfile: the JSON file `helper generate-key` writes, or
    /// a plain secret seed phrase or derivation path (e.g., "//Alice"). The
    /// file must not be group- or world-readable (permissions <= 0600 on
    /// Unix).
    #[arg(long, value_name = "FILE")]
    pub keyfile: Option<PathBuf>,

    /// Provider account ID (SS58). Used when --keyfile is not set; disables
    /// signing capability.
    #[arg(long, value_name = "ACCOUNT", env = "PROVIDER_ID")]
    pub provider_id: Option<String>,

    /// Signature scheme for the signing keypair derived from --keyfile — must
    /// match the scheme of the `public_key` registered on-chain. Defaults to
    /// the scheme recorded in a JSON keyfile, else sr25519. The
    /// extrinsic-submission account stays sr25519 regardless.
    #[arg(long, value_enum)]
    pub key_scheme: Option<provider_types::KeyScheme>,
}

/// The secret loaded from a keyfile and the scheme to sign with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedKey {
    /// The seed phrase or derivation path.
    pub seed: String,
    /// The signing scheme: `--key-scheme` when given, else the keyfile's
    /// `keyScheme`, else sr25519.
    pub scheme: provider_types::KeyScheme,
}

/// Read a keyfile and resolve the signing scheme. A file starting with `{`
/// is the JSON keyfile `helper generate-key` writes; any other content is
/// the seed itself.
fn load_key(
    path: &std::path::Path,
    scheme_flag: Option<provider_types::KeyScheme>,
) -> Result<LoadedKey, String> {
    let contents = read_secret_file(path)?;
    if !contents.starts_with('{') {
        return Ok(LoadedKey {
            seed: contents,
            scheme: scheme_flag.unwrap_or_default(),
        });
    }
    let keyfile = provider_helpers::Keyfile::from_json(&contents)
        .map_err(|e| format!("Keyfile {}: {e}", path.display()))?;
    if keyfile.secret_phrase.trim().is_empty() {
        return Err(format!(
            "Keyfile {} has an empty secretPhrase",
            path.display()
        ));
    }
    let file_scheme = match &keyfile.key_scheme {
        Some(name) => <provider_types::KeyScheme as clap::ValueEnum>::from_str(name, true)
            .map_err(|_| format!("Keyfile {} has unknown keyScheme {name:?}", path.display()))?,
        None => provider_types::KeyScheme::default(),
    };
    Ok(LoadedKey {
        seed: keyfile.secret_phrase,
        scheme: scheme_flag.unwrap_or(file_scheme),
    })
}

/// The name of `scheme` as `--key-scheme` and a keyfile's `keyScheme`
/// spell it: the clap value name, so what is written is what `load_key`
/// parses.
pub fn key_scheme_name(scheme: provider_types::KeyScheme) -> String {
    <provider_types::KeyScheme as clap::ValueEnum>::to_possible_value(&scheme)
        .expect("every KeyScheme variant has a clap value")
        .get_name()
        .to_string()
}

/// Signing key of the `helper` subcommand. Every helper operation signs a
/// transaction with it or, for `generate-key`, creates it, so the keyfile is
/// required.
#[derive(Clone, Debug, clap::Args)]
pub struct HelperKeyParams {
    /// Path to the keyfile: the JSON file `helper generate-key` writes, or
    /// a plain secret seed phrase or derivation path (e.g., "//Alice"). The
    /// file must not be group- or world-readable (permissions <= 0600 on
    /// Unix). `generate-key` creates it.
    #[arg(long, value_name = "FILE")]
    pub keyfile: PathBuf,

    /// Signature scheme of the signing keypair registered as the on-chain
    /// `public_key`. The helper supports sr25519 only for now and rejects
    /// any other value, from this flag or from a keyfile's `keyScheme`.
    #[arg(long, value_enum)]
    pub key_scheme: Option<provider_types::KeyScheme>,
}

impl HelperKeyParams {
    /// The seed and scheme from `--keyfile`, with the same permission checks
    /// as the node's `--keyfile`. Rejects a scheme other than sr25519.
    pub fn load(&self) -> Result<LoadedKey, String> {
        let key = load_key(&self.keyfile, self.key_scheme)?;
        helper_scheme(key.scheme)?;
        Ok(key)
    }

    /// The scheme `generate-key` records: `--key-scheme` when given, else
    /// sr25519. Rejects any other scheme.
    pub fn generate_scheme(&self) -> Result<provider_types::KeyScheme, String> {
        helper_scheme(self.key_scheme.unwrap_or_default())
    }
}

/// The helper supports sr25519 only for now.
fn helper_scheme(scheme: provider_types::KeyScheme) -> Result<provider_types::KeyScheme, String> {
    match scheme {
        provider_types::KeyScheme::Sr25519 => Ok(scheme),
        other => Err(format!(
            "key scheme {} is not supported by the helper yet; only sr25519 is",
            key_scheme_name(other)
        )),
    }
}

impl KeyParams {
    /// Resolve the signing key from CLI parameters.
    ///
    /// Priority:
    /// 1. `--keyfile` reads the file contents (with permission checks)
    /// 2. No keyfile returns `None` (provider-id mode without signing)
    pub fn load(&self) -> Result<Option<LoadedKey>, String> {
        let Some(ref path) = self.keyfile else {
            return Ok(None);
        };

        load_key(path, self.key_scheme).map(Some)
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

    /// Maximum age of cached membership served while the chain is unreachable.
    #[arg(
        long,
        value_name = "SECONDS",
        default_value_t = 300,
        env = "AUTH_MAX_STALE"
    )]
    pub auth_max_stale: u64,

    /// Maximum number of buckets' membership resident in the cache at once.
    #[arg(
        long,
        value_name = "ENTRIES",
        default_value_t = 10_000,
        env = "AUTH_CACHE_MAX_ENTRIES"
    )]
    pub auth_cache_max_entries: u64,
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
        assert_eq!(cli.chain.chain_rpc, "ws://127.0.0.1:2222");
        assert!(cli.command.is_none());
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
        assert_eq!(cli.chain.chain_rpc, "ws://example.com:9944");
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
        assert!(matches!(cli.chain.chain_transport, TransportKind::Rpc));
        let transport = cli.chain.chain_transport().unwrap();
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
        } = cli.chain.chain_transport().unwrap()
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
        } = cli.chain.chain_transport().unwrap()
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
        let err = cli.chain.chain_transport().unwrap_err();
        assert!(
            err.contains("--relay-chain-spec"),
            "unexpected error: {err}"
        );
    }

    fn helper(args: &[&str]) -> Result<(ChainParams, HelperCommand), clap::Error> {
        let mut argv = vec!["storage-provider-node", "helper"];
        argv.extend_from_slice(args);
        Cli::try_parse_from(argv).map(|cli| match cli.command {
            Some(Command::Helper(cmd)) => (cli.chain, cmd),
            None => panic!("expected the helper subcommand"),
        })
    }

    #[test]
    fn helper_register_parses() {
        let (chain, cmd) = helper(&[
            "--keyfile",
            "/tmp/test-key",
            "--chain-rpc",
            "ws://example.com:9944",
            "--key-scheme",
            "ecdsa",
            "register",
            "--multiaddr",
            "/ip4/1.2.3.4/tcp/3333",
            "--stake",
            "1000000000000000",
        ])
        .unwrap();
        assert_eq!(cmd.key.keyfile, PathBuf::from("/tmp/test-key"));
        assert_eq!(cmd.key.key_scheme, Some(provider_types::KeyScheme::Ecdsa));
        // Parsed, but rejected when used: the helper is sr25519-only.
        let err = cmd.key.generate_scheme().unwrap_err();
        assert!(err.contains("ecdsa") && err.contains("sr25519"), "{err}");
        assert_eq!(chain.chain_rpc, "ws://example.com:9944");
        let HelperAction::Register { multiaddr, stake } = cmd.action else {
            panic!("expected register");
        };
        assert_eq!(multiaddr, "/ip4/1.2.3.4/tcp/3333");
        assert_eq!(stake, 1_000_000_000_000_000);
    }

    /// The chain flags are global: given before `helper`, they reach the
    /// same `cli.chain` the helper reads.
    #[test]
    fn chain_flags_before_helper_apply() {
        let cli = Cli::try_parse_from([
            "storage-provider-node",
            "--chain-rpc",
            "ws://example.com:9944",
            "--chain-transport",
            "light",
            "helper",
            "--keyfile",
            "/tmp/k",
            "deregister",
        ])
        .unwrap();
        assert!(matches!(cli.command, Some(Command::Helper(_))));
        assert_eq!(cli.chain.chain_rpc, "ws://example.com:9944");
        assert_eq!(cli.chain.chain_transport, TransportKind::Light);
    }

    /// The helper signs every transaction, so it needs a keyfile; the
    /// register action needs its multiaddr and stake.
    #[test]
    fn helper_requires_keyfile_and_register_arguments() {
        assert!(helper(&[
            "register",
            "--multiaddr",
            "/ip4/1.2.3.4/tcp/3333",
            "--stake",
            "1"
        ])
        .is_err());
        assert!(helper(&["--keyfile", "/tmp/k", "register", "--stake", "1"]).is_err());
        assert!(helper(&[
            "--keyfile",
            "/tmp/k",
            "register",
            "--multiaddr",
            "/ip4/1.2.3.4/tcp/3333"
        ])
        .is_err());
        assert!(helper(&["--keyfile", "/tmp/k", "deregister"]).is_ok());
    }

    #[test]
    fn helper_generate_key_parses() {
        let (_, cmd) = helper(&["--keyfile", "/tmp/k", "generate-key"]).unwrap();
        assert!(matches!(
            cmd.action,
            HelperAction::GenerateKey { words: 12 }
        ));

        let (_, cmd) = helper(&["--keyfile", "/tmp/k", "generate-key", "--words", "24"]).unwrap();
        assert!(matches!(
            cmd.action,
            HelperAction::GenerateKey { words: 24 }
        ));

        // Only the BIP39 word counts parse.
        for rejected in ["13", "0", "twelve"] {
            assert!(
                helper(&["--keyfile", "/tmp/k", "generate-key", "--words", rejected]).is_err(),
                "--words {rejected} should be rejected"
            );
        }
    }

    #[test]
    fn helper_update_settings_maps_flags_to_a_partial_update() {
        let (_, cmd) = helper(&["--keyfile", "/tmp/k", "update-settings"]).unwrap();
        let HelperAction::UpdateSettings(args) = cmd.action else {
            panic!("expected update-settings");
        };
        assert_eq!(
            args.update(),
            provider_helpers::ProviderSettingsUpdate::default()
        );

        let (_, cmd) = helper(&[
            "--keyfile",
            "/tmp/k",
            "update-settings",
            "--min-duration",
            "10",
            "--price-per-byte",
            "7",
            "--accepting-primary",
            "false",
            "--replica-sync-price",
            "3",
            "--max-capacity",
            "1024",
        ])
        .unwrap();
        let HelperAction::UpdateSettings(args) = cmd.action else {
            panic!("expected update-settings");
        };
        assert_eq!(
            args.update(),
            provider_helpers::ProviderSettingsUpdate {
                min_duration: Some(10),
                price_per_byte: Some(7),
                accepting_primary: Some(false),
                replica_sync_price: Some(Some(3)),
                max_capacity: Some(1024),
                ..Default::default()
            }
        );

        let (_, cmd) =
            helper(&["--keyfile", "/tmp/k", "update-settings", "--no-replicas"]).unwrap();
        let HelperAction::UpdateSettings(args) = cmd.action else {
            panic!("expected update-settings");
        };
        assert_eq!(args.update().replica_sync_price, Some(None));

        // A duration above the chain's u32 block range is rejected by clap.
        assert!(helper(&[
            "--keyfile",
            "/tmp/k",
            "update-settings",
            "--max-duration",
            "4294967296"
        ])
        .is_err());

        // Setting and clearing the replica price at once is contradictory.
        assert!(helper(&[
            "--keyfile",
            "/tmp/k",
            "update-settings",
            "--no-replicas",
            "--replica-sync-price",
            "3"
        ])
        .is_err());
    }

    fn key_params(keyfile: Option<PathBuf>) -> KeyParams {
        KeyParams {
            keyfile,
            provider_id: None,
            key_scheme: None,
        }
    }

    #[test]
    fn load_seed_missing_file() {
        let params = key_params(Some(PathBuf::from("/nonexistent/path/to/keyfile")));
        let err = params.load().unwrap_err();
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

        let err = key_params(Some(path)).load().unwrap_err();
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

        // A plain seed file: the scheme is the flag, else sr25519.
        assert_eq!(
            key_params(Some(path.clone())).load().unwrap(),
            Some(LoadedKey {
                seed: "//Charlie".to_string(),
                scheme: provider_types::KeyScheme::Sr25519,
            })
        );
        let params = KeyParams {
            key_scheme: Some(provider_types::KeyScheme::Eth),
            ..key_params(Some(path))
        };
        assert_eq!(
            params.load().unwrap().unwrap().scheme,
            provider_types::KeyScheme::Eth
        );
    }

    /// A JSON keyfile supplies the seed and, unless `--key-scheme` is given,
    /// the scheme.
    #[test]
    fn load_seed_from_json_keyfile() {
        let path = std::env::temp_dir().join("cli-test-json-keyfile");
        std::fs::write(
            &path,
            r#"{"secretPhrase": "//Dave", "keyScheme": "ecdsa", "ss58Address": "5abc"}"#,
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }

        assert_eq!(
            key_params(Some(path.clone())).load().unwrap(),
            Some(LoadedKey {
                seed: "//Dave".to_string(),
                scheme: provider_types::KeyScheme::Ecdsa,
            })
        );
        let params = KeyParams {
            key_scheme: Some(provider_types::KeyScheme::Ed25519),
            ..key_params(Some(path.clone()))
        };
        assert_eq!(
            params.load().unwrap().unwrap().scheme,
            provider_types::KeyScheme::Ed25519
        );

        // The helper rejects the recorded non-sr25519 scheme; the node
        // accepts it.
        let helper = HelperKeyParams {
            keyfile: path.clone(),
            key_scheme: None,
        };
        let err = helper.load().unwrap_err();
        assert!(err.contains("ecdsa"), "unexpected error: {err}");
        let helper = HelperKeyParams {
            keyfile: path.clone(),
            key_scheme: Some(provider_types::KeyScheme::Sr25519),
        };
        assert_eq!(helper.load().unwrap().seed, "//Dave");

        // No keyScheme (as subkey writes it): sr25519.
        std::fs::write(
            &path,
            r#"{"secretPhrase": "//Dave", "networkId": "substrate", "secretSeed": "0x00"}"#,
        )
        .unwrap();
        assert_eq!(
            key_params(Some(path.clone())).load().unwrap(),
            Some(LoadedKey {
                seed: "//Dave".to_string(),
                scheme: provider_types::KeyScheme::Sr25519,
            })
        );

        std::fs::write(&path, r#"{"secretPhrase": "  "}"#).unwrap();
        let err = key_params(Some(path.clone())).load().unwrap_err();
        assert!(
            err.contains("empty secretPhrase"),
            "unexpected error: {err}"
        );

        std::fs::write(&path, r#"{"secretPhrase": "//Dave", "keyScheme": "rsa"}"#).unwrap();
        let err = key_params(Some(path.clone())).load().unwrap_err();
        assert!(err.contains("unknown keyScheme"), "unexpected error: {err}");

        std::fs::write(&path, r#"{"keyScheme": "ecdsa"}"#).unwrap();
        let err = key_params(Some(path)).load().unwrap_err();
        assert!(err.contains("not valid JSON"), "unexpected error: {err}");
    }

    #[cfg(unix)]
    #[test]
    fn load_seed_rejects_world_readable() {
        use std::os::unix::fs::PermissionsExt;

        let path = std::env::temp_dir().join("cli-test-insecure-keyfile");
        std::fs::write(&path, "//Alice").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        let err = key_params(Some(path)).load().unwrap_err();
        assert!(
            err.contains("insecure permissions"),
            "unexpected error: {err}"
        );
    }

    /// The written scheme name is the clap value name, for every scheme.
    #[test]
    fn key_scheme_names_round_trip() {
        for scheme in [
            provider_types::KeyScheme::Sr25519,
            provider_types::KeyScheme::Ed25519,
            provider_types::KeyScheme::Ecdsa,
            provider_types::KeyScheme::Eth,
        ] {
            let name = key_scheme_name(scheme);
            assert_eq!(
                <provider_types::KeyScheme as clap::ValueEnum>::from_str(&name, true).unwrap(),
                scheme
            );
        }
        assert_eq!(
            key_scheme_name(provider_types::KeyScheme::Sr25519),
            "sr25519"
        );
    }

    #[test]
    fn load_seed_none_without_keyfile() {
        assert_eq!(key_params(None).load().unwrap(), None);
    }
}
