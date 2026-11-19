// SPDX-License-Identifier: Apache-2.0

//! Command-line argument parsing for the storage CLI.

use crate::commands::provider::Provider;
use crate::commands::soak::Soak;
use crate::commands::stress_test::StressTest;
use crate::metrics::OutputFormat;
use clap::{Args, Parser, Subcommand};

/// Storage CLI for scalable Web3 storage — drive on-chain and off-chain
/// storage operations from a single tool.
#[derive(Debug, Parser)]
#[command(name = "storage-cli", version, about)]
pub struct Cli {
    #[clap(flatten)]
    pub global: GlobalArgs,

    #[command(subcommand)]
    pub command: Command,
}

/// Connection and output flags shared by every subcommand. Each subcommand
/// takes the secret of the account it signs as.
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

    /// Format for the final metrics summary printed to stdout. Commands
    /// without metrics ignore it.
    #[arg(long, value_enum, value_name = "FORMAT", default_value_t = OutputFormat::Text)]
    pub output: OutputFormat,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Stress-test operations against a provider.
    #[command(subcommand)]
    StressTest(StressTest),

    /// Provider registration.
    #[command(subcommand)]
    Provider(Provider),

    /// Long-running correctness tests of the provider's background services.
    #[command(subcommand)]
    Soak(Soak),
}
