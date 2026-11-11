// SPDX-License-Identifier: Apache-2.0

//! `storage-cli` — operator CLI for scalable Web3 storage.
//!
//! Drives on-chain and off-chain storage operations through the
//! [`storage-client`](../../client) SDK. See `--help` for the available
//! subcommands.

mod actions;
mod cli;
mod commands;
mod common;
mod metrics;
mod prepare;

use anyhow::bail;
use clap::Parser;

use crate::cli::{Cli, Command};
use crate::commands::stress_test::StressTest;
use crate::metrics::{print_text, to_json, OpSummary, OutputFormat};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    // Scenarios return their metrics and what their setup phase created;
    // `main` only renders them, so every subcommand shares one output path.
    let (summary, preparation) = match &cli.command {
        Command::StressTest(StressTest::ProviderUpload(args)) => {
            commands::stress_test::upload(&cli.global, args).await?
        }
    };
    let all_results: Vec<OpSummary> = vec![summary];

    match cli.global.output {
        OutputFormat::Text => {
            if let Some(report) = &preparation {
                print!("{report}");
            }
            print_text(&all_results)
        }
        OutputFormat::Json => println!("{}", to_json(&all_results, preparation.as_ref())?),
    }

    // Non-zero exit if any scenario completed with no successful operations.
    if let Some(m) = all_results.iter().find(|m| m.total > 0 && m.ok == 0) {
        bail!("all {} {} failed", m.total, m.labels.noun_plural);
    }

    Ok(())
}
