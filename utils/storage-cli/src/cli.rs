// SPDX-License-Identifier: Apache-2.0

//! Command-line argument parsing for the storage CLI.

use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;
use crate::commands::stress_test::StressTest;

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

