//! Constants and environment variable helpers for zombienet-sdk tests.

// Environment variable names
pub const POLKADOT_BINARY_PATH_ENV: &str = "POLKADOT_BINARY_PATH";
pub const POLKADOT_OMNI_NODE_PATH_ENV: &str = "POLKADOT_OMNI_NODE_PATH";
pub const CHAIN_SPEC_COMMAND_ENV: &str = "CHAIN_SPEC_COMMAND";
pub const PROVIDER_BINARY_PATH_ENV: &str = "PROVIDER_BINARY_PATH";

// Default binary paths
pub const DEFAULT_POLKADOT_BINARY: &str = ".bin/polkadot";
pub const DEFAULT_OMNI_NODE_BINARY: &str = ".bin/polkadot-omni-node";
pub const DEFAULT_PROVIDER_BINARY: &str = "./target/release/storage-provider-node";

// Chain spec command
pub const DEFAULT_CHAIN_SPEC_COMMAND: &str = "./scripts/build-chain-spec.sh";

// Network parameters
pub const RELAY_CHAIN: &str = "westend-local";
pub const PARA_ID: u32 = 4000;
pub const PROVIDER_PORT: u16 = 3333;

// Timeouts (seconds)
pub const PROVIDER_HEALTH_TIMEOUT_SECS: u64 = 60;
pub const PROVIDER_HEALTH_POLL_INTERVAL_MS: u64 = 2000;

// Provider URLs
pub const PROVIDER_BIND_ADDR: &str = "0.0.0.0:3333";

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

pub fn provider_url() -> String {
    format!("http://127.0.0.1:{}", PROVIDER_PORT)
}
