// SPDX-License-Identifier: Apache-2.0

//! Command-line argument parsing for the storage CLI.

use crate::commands::provider::ProviderCommand;
use crate::commands::stress_test::StressTest;
use crate::metrics::OutputFormat;
use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

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

    /// Path to the keyfile: the JSON file `provider generate-key` writes,
    /// or a file whose contents are the SURI/seed for the account. The file
    /// must not be group- or world-readable (permissions <= 0600 on Unix).
    /// Mutually exclusive with `--suri`.
    #[arg(long, value_name = "FILE", conflicts_with = "suri")]
    pub keyfile: Option<PathBuf>,

    /// Format for the final metrics summary printed to stdout.
    #[arg(long, value_enum, value_name = "FORMAT", default_value_t = OutputFormat::Text)]
    pub output: OutputFormat,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Stress-test operations against a provider.
    #[command(subcommand)]
    StressTest(StressTest),

    /// Manage a provider's on-chain registration with the `--suri` or
    /// `--keyfile` account: generate its keyfile, register, update settings,
    /// deregister.
    Provider(ProviderCommand),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::provider::ProviderAction;
    use provider_types::KeyScheme;

    fn provider(args: &[&str]) -> Result<(GlobalArgs, ProviderCommand), clap::Error> {
        let mut argv = vec!["storage-cli"];
        argv.extend_from_slice(args);
        Cli::try_parse_from(argv).map(|cli| match cli.command {
            Command::Provider(cmd) => (cli.global, cmd),
            other => panic!("expected the provider subcommand, got {other:?}"),
        })
    }

    #[test]
    fn provider_register_parses() {
        let (global, cmd) = provider(&[
            "--keyfile",
            "/tmp/test-key",
            "--chain-rpc",
            "ws://example.com:9944",
            "provider",
            "--key-scheme",
            "ecdsa",
            "register",
            "--multiaddr",
            "/ip4/1.2.3.4/tcp/3333",
            "--stake",
            "1000000000000000",
        ])
        .unwrap();
        assert_eq!(global.keyfile, Some(PathBuf::from("/tmp/test-key")));
        assert_eq!(global.chain_rpc, "ws://example.com:9944");
        assert_eq!(cmd.key_scheme, Some(KeyScheme::Ecdsa));
        let ProviderAction::Register { multiaddr, stake } = cmd.action else {
            panic!("expected register");
        };
        assert_eq!(multiaddr, "/ip4/1.2.3.4/tcp/3333");
        assert_eq!(stake, 1_000_000_000_000_000);
    }

    /// The register action needs its multiaddr and stake.
    #[test]
    fn provider_register_requires_arguments() {
        assert!(provider(&["provider", "register", "--stake", "1"]).is_err());
        assert!(provider(&[
            "provider",
            "register",
            "--multiaddr",
            "/ip4/1.2.3.4/tcp/3333"
        ])
        .is_err());
        assert!(provider(&["provider", "deregister"]).is_ok());
    }

    #[test]
    fn provider_generate_key_parses() {
        let (_, cmd) = provider(&["provider", "generate-key"]).unwrap();
        assert!(matches!(
            cmd.action,
            ProviderAction::GenerateKey { words: 12 }
        ));

        let (_, cmd) = provider(&["provider", "generate-key", "--words", "24"]).unwrap();
        assert!(matches!(
            cmd.action,
            ProviderAction::GenerateKey { words: 24 }
        ));

        // Only the BIP39 word counts parse.
        for rejected in ["13", "0", "twelve"] {
            assert!(
                provider(&["provider", "generate-key", "--words", rejected]).is_err(),
                "--words {rejected} should be rejected"
            );
        }
    }

    #[test]
    fn provider_update_settings_parses() {
        let (_, cmd) = provider(&[
            "provider",
            "update-settings",
            "--min-duration",
            "10",
            "--no-replicas",
        ])
        .unwrap();
        let ProviderAction::UpdateSettings(args) = cmd.action else {
            panic!("expected update-settings");
        };
        assert_eq!(args.min_duration, Some(10));
        assert!(args.no_replicas);

        // A duration above the chain's u32 block range is rejected by clap.
        assert!(provider(&[
            "provider",
            "update-settings",
            "--max-duration",
            "4294967296"
        ])
        .is_err());

        // Setting and clearing the replica price at once is contradictory.
        assert!(provider(&[
            "provider",
            "update-settings",
            "--no-replicas",
            "--replica-sync-price",
            "3"
        ])
        .is_err());
    }
}
