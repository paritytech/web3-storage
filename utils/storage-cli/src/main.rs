//! `storage-cli` — operator CLI for scalable Web3 storage.
//!
//! Drives on-chain and off-chain storage operations through the
//! [`storage-client`](../../client) SDK. See `--help` for the available
//! subcommands.

mod cli;
mod commands;

use clap::Parser;

use crate::cli::{Cli, Command, StressTest};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();
    match &cli.command {
        Command::StressTest(StressTest::Upload(args)) => {
            commands::stress_test::upload(&cli.global, args).await
        }
    }
}
