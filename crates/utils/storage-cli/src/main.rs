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
#[allow(dead_code)]
mod soak;

use anyhow::bail;
use clap::Parser;

use crate::cli::{Cli, Command};
use crate::commands::provider::Provider;
use crate::commands::soak::Soak;
use crate::commands::stress_test::StressTest;
use crate::metrics::{print_text, to_json, OpSummary, OutputFormat};
use crate::prepare::PreparationReport;
use crate::soak::SoakReport;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    match &cli.command {
        Command::StressTest(StressTest::ProviderUpload(args)) => {
            let (summary, preparation) = commands::stress_test::upload(&cli.global, args).await?;
            render_metrics(cli.global.output, summary, preparation)
        }
        Command::Provider(Provider::Register(args)) => {
            commands::provider::register(&cli.global, args).await
        }
        Command::Soak(Soak::Gc(args)) => {
            let report = commands::soak::gc::run(&cli.global, args).await?;
            render_soak(cli.global.output, &report)
        }
    }
}

/// Print a soak report. Fails if the run recorded a failure.
fn render_soak(output: OutputFormat, report: &SoakReport) -> anyhow::Result<()> {
    match output {
        OutputFormat::Text => print!("{report}"),
        OutputFormat::Json => println!("{}", report.to_json()?),
    }
    match &report.failure {
        Some(failure) => bail!("{failure}"),
        None => Ok(()),
    }
}

/// Print what a scenario's setup phase created and its metrics. Fails if
/// every operation of the scenario failed.
fn render_metrics(
    output: OutputFormat,
    summary: OpSummary,
    preparation: Option<PreparationReport>,
) -> anyhow::Result<()> {
    let all_results: Vec<OpSummary> = vec![summary];

    match output {
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
