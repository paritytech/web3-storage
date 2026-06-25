// SPDX-License-Identifier: Apache-2.0
//! `storage-cli` — operator CLI for scalable Web3 storage.
//!
//! Drives on-chain and off-chain storage operations through the
//! [`storage-client`](../../client) SDK. See `--help` for the available
//! subcommands.

mod cli;
mod commands;
mod common;
mod metrics;

use anyhow::bail;
use clap::Parser;

use crate::cli::{Cli, Command};
use crate::commands::stress_test::StressTest;
use crate::metrics::{print_text, to_json, OpSummary, OutputFormat};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    // Scenarios compute and return their metrics; `main` collects every run's
    // results and views them here. Adding a `read`/`delete` subcommand means
    // pushing another `OpSummary` onto this vec — the viewing below is shared.
    let mut all_results: Vec<OpSummary> = Vec::new();
    match &cli.command {
        Command::StressTest(StressTest::ProviderUpload(args)) => {
            all_results.push(commands::stress_test::upload(&cli.global, args).await?);
        }
    }

    match cli.global.output {
        OutputFormat::Text => print_text(&all_results),
        OutputFormat::Json => println!("{}", to_json(&all_results)?),
    }

    // Non-zero exit if any scenario completed with no successful operations.
    if let Some(m) = all_results.iter().find(|m| m.total > 0 && m.ok == 0) {
        bail!("all {} {} failed", m.total, m.operation.noun_plural());
    }

    Ok(())
}
