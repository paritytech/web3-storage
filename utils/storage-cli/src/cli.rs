//! Command-line argument parsing for the storage operator CLI.

use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

/// Operator CLI for scalable Web3 storage — drive on-chain and off-chain
/// storage operations from a single tool.
#[derive(Debug, Parser)]
#[command(name = "storage-cli", version, about)]
pub struct Cli {
    #[clap(flatten)]
    pub global: GlobalArgs,

    #[command(subcommand)]
    pub command: Command,
}

/// Connection and identity flags shared by every subcommand.
#[derive(Debug, Args)]
pub struct GlobalArgs {
    /// WebSocket URL for the parachain RPC.
    #[arg(
        long,
        value_name = "URL",
        default_value = "ws://127.0.0.1:2222",
        env = "CHAIN_RPC"
    )]
    pub chain_rpc: String,

    /// HTTP URL of the provider node.
    #[arg(
        long,
        value_name = "URL",
        default_value = "http://127.0.0.1:3333",
        env = "PROVIDER_URL"
    )]
    pub provider_url: String,

    /// Secret URI (SURI) for the signing/identity account, e.g. "//Alice".
    /// Mutually exclusive with `--keyfile`.
    #[arg(long, value_name = "SURI", conflicts_with = "keyfile")]
    pub suri: Option<String>,

    /// Path to a file whose contents are the SURI/seed for the account.
    /// Mutually exclusive with `--suri`.
    #[arg(long, value_name = "FILE", conflicts_with = "suri")]
    pub keyfile: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Stress-test operations against a provider.
    #[command(subcommand)]
    StressTest(StressTest),
}

#[derive(Debug, Subcommand)]
pub enum StressTest {
    /// Upload generated data to every bucket the account already has an
    /// agreement with the given provider for.
    Upload(UploadArgs),
}

#[derive(Debug, Args)]
pub struct UploadArgs {
    /// Provider account (SS58 or 0x-hex) whose agreements select the target
    /// buckets.
    #[arg(long, value_name = "ACCOUNT")]
    pub provider: String,

    /// Cap the number of buckets written to (default: all matching buckets).
    #[arg(long, value_name = "N")]
    pub max_buckets_to_write: Option<usize>,

    /// Bytes of generated data to upload per bucket.
    #[arg(long, value_name = "BYTES", default_value_t = 1024 * 1024)]
    pub size: usize,
}
