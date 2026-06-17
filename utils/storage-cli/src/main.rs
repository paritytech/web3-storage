//! `storage-cli` — operator CLI for scalable Web3 storage.
//!
//! Drives on-chain and off-chain storage operations through the
//! [`storage-client`](../../client) SDK. See `--help` for the available
//! subcommands.

mod cli_args;
mod shared;
mod scenarios;

use clap::Parser;

use crate::cli_args::{Cli, Command, StressTest};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();
    match &cli.command {
        Command::StressTest(StressTest::Upload(args)) => {
            scenarios::stress_test::upload(&cli.global, args).await
        }
    }
}
