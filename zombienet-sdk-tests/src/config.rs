//! Constants and environment variable helpers for zombienet-sdk tests.

// Environment variable names
pub const POLKADOT_BINARY_PATH_ENV: &str = "POLKADOT_BINARY_PATH";
pub const POLKADOT_OMNI_NODE_PATH_ENV: &str = "POLKADOT_OMNI_NODE_PATH";
pub const CHAIN_SPEC_COMMAND_ENV: &str = "CHAIN_SPEC_COMMAND";
pub const PROVIDER_BINARY_PATH_ENV: &str = "PROVIDER_BINARY_PATH";
pub const RELAY_RPC_PORT_ENV: &str = "RELAY_RPC_PORT";
pub const CHAIN_RPC_PORT_ENV: &str = "CHAIN_RPC_PORT";
pub const PROVIDER_PORT_ENV: &str = "PROVIDER_PORT";

// Default binary paths
pub const DEFAULT_POLKADOT_BINARY: &str = ".bin/polkadot";
pub const DEFAULT_OMNI_NODE_BINARY: &str = ".bin/polkadot-omni-node";
pub const DEFAULT_PROVIDER_BINARY: &str = "./target/release/storage-provider-node";

// Chain spec command
pub const DEFAULT_CHAIN_SPEC_COMMAND: &str = "./scripts/build-chain-spec.sh";

// Network parameters
pub const RELAY_CHAIN: &str = "westend-local";
pub const PARA_ID: u32 = 4000;
pub const DEFAULT_RELAY_RPC_PORT: u16 = 9900;
pub const DEFAULT_CHAIN_RPC_PORT: u16 = 2222;
pub const DEFAULT_PROVIDER_PORT: u16 = 3333;

// Timeouts
pub const PROVIDER_HEALTH_TIMEOUT_SECS: u64 = 60;
pub const PROVIDER_HEALTH_POLL_INTERVAL_MS: u64 = 2000;
pub const CLIENT_TIMEOUT_SECS: u64 = 60;

// Well-known dev accounts (SS58)
pub const ALICE_SS58: &str = "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY";
pub const BOB_SS58: &str = "5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty";

// Alice's sr25519 public key hex (well-known dev account)
pub const ALICE_PUBLIC_KEY_HEX: &str =
    "d43593c715fdd31c61141abd04a99fd6822c8558854ccde39a5684e7a56da27d";

// On-chain test parameters
pub const PROVIDER_STAKE: u128 = 1_000_000_000_000_000; // 1000 tokens (12 decimals)
pub const BUCKET_ID: u64 = 0; // first bucket on a fresh chain
pub const AGREEMENT_MAX_BYTES: u64 = 1_073_741_824; // 1 GB
pub const AGREEMENT_DURATION_BLOCKS: u32 = 100_000;
pub const AGREEMENT_MAX_PAYMENT: u128 = 100_000_000_000;

/// Workspace root directory (resolved from CARGO_MANIFEST_DIR).
pub fn workspace_root() -> std::path::PathBuf {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
    let root = std::path::Path::new(&manifest_dir)
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .to_path_buf();
    std::fs::canonicalize(&root).unwrap_or(root)
}

/// Resolve a path: if it starts with "./" or ".bin/", make it absolute relative to workspace root.
pub fn resolve_path(path: &str) -> String {
    if path.starts_with("./") || path.starts_with(".bin/") {
        let abs = workspace_root().join(path);
        abs.to_string_lossy().to_string()
    } else {
        path.to_string()
    }
}

pub fn env_or_default(var: &str, default: &str) -> String {
    std::env::var(var).unwrap_or_else(|_| default.to_string())
}

pub fn get_polkadot_binary_path() -> String {
    resolve_path(&env_or_default(
        POLKADOT_BINARY_PATH_ENV,
        DEFAULT_POLKADOT_BINARY,
    ))
}

pub fn get_omni_node_binary_path() -> String {
    resolve_path(&env_or_default(
        POLKADOT_OMNI_NODE_PATH_ENV,
        DEFAULT_OMNI_NODE_BINARY,
    ))
}

pub fn get_chain_spec_command() -> String {
    resolve_path(&env_or_default(
        CHAIN_SPEC_COMMAND_ENV,
        DEFAULT_CHAIN_SPEC_COMMAND,
    ))
}

pub fn get_provider_binary_path() -> String {
    resolve_path(&env_or_default(
        PROVIDER_BINARY_PATH_ENV,
        DEFAULT_PROVIDER_BINARY,
    ))
}

fn env_port(var: &str, default: u16) -> u16 {
    std::env::var(var)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

pub fn relay_rpc_port() -> u16 {
    env_port(RELAY_RPC_PORT_ENV, DEFAULT_RELAY_RPC_PORT)
}

pub fn chain_rpc_port() -> u16 {
    env_port(CHAIN_RPC_PORT_ENV, DEFAULT_CHAIN_RPC_PORT)
}

pub fn provider_port() -> u16 {
    env_port(PROVIDER_PORT_ENV, DEFAULT_PROVIDER_PORT)
}

pub fn provider_url() -> String {
    let port = provider_port();
    format!("http://127.0.0.1:{port}")
}

pub fn provider_bind_addr() -> String {
    let port = provider_port();
    format!("0.0.0.0:{port}")
}

pub fn provider_multiaddr() -> String {
    let port = provider_port();
    format!("/ip4/127.0.0.1/tcp/{port}")
}
