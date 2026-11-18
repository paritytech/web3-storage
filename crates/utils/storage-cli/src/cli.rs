// SPDX-License-Identifier: Apache-2.0

//! Command-line argument parsing for the storage CLI.

use crate::commands::provider::ProviderAction;
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

    /// Manage a provider's on-chain registration with the
    /// `--provider-suri` or `--provider-keyfile` account: generate its
    /// keyfile, register, update settings, deregister.
    #[command(subcommand)]
    Provider(ProviderAction),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::provider::MIN_PROVIDER_STAKE;
    use provider_types::KeyScheme;
    use std::path::PathBuf;

    fn provider(args: &[&str]) -> Result<(GlobalArgs, ProviderAction), clap::Error> {
        let mut argv = vec!["storage-cli"];
        argv.extend_from_slice(args);
        Cli::try_parse_from(argv).map(|cli| match cli.command {
            Command::Provider(action) => (cli.global, action),
            other => panic!("expected the provider subcommand, got {other:?}"),
        })
    }

    #[test]
    fn provider_register_parses() {
        let (global, action) = provider(&[
            "--chain-rpc",
            "ws://example.com:9944",
            "provider",
            "register",
            "--provider-keyfile",
            "/tmp/test-key",
            "--key-scheme",
            "ecdsa",
            "--multiaddr",
            "/ip4/1.2.3.4/tcp/3333",
            "--stake",
            "2000000000000000",
        ])
        .unwrap();
        assert_eq!(global.chain_rpc, "ws://example.com:9944");
        let ProviderAction::Register(args) = action else {
            panic!("expected register");
        };
        assert_eq!(
            args.provider.provider_keyfile,
            Some(PathBuf::from("/tmp/test-key"))
        );
        assert_eq!(args.key_scheme, Some(KeyScheme::Ecdsa));
        assert_eq!(args.multiaddr, "/ip4/1.2.3.4/tcp/3333");
        assert_eq!(args.stake, 2_000_000_000_000_000);
    }

    /// The register action needs the provider key and its multiaddr; the
    /// stake defaults to the minimum provider stake.
    #[test]
    fn provider_register_requires_arguments() {
        assert!(provider(&[
            "provider",
            "register",
            "--multiaddr",
            "/ip4/1.2.3.4/tcp/3333"
        ])
        .is_err());
        assert!(provider(&["provider", "register", "--provider-suri", "//Alice"]).is_err());
        let (_, action) = provider(&[
            "provider",
            "register",
            "--provider-suri",
            "//Alice",
            "--multiaddr",
            "/ip4/1.2.3.4/tcp/3333",
        ])
        .unwrap();
        let ProviderAction::Register(args) = action else {
            panic!("expected register");
        };
        assert_eq!(args.stake, MIN_PROVIDER_STAKE);
        assert_eq!(args.key_scheme, None);

        assert!(provider(&["provider", "deregister"]).is_err());
        assert!(provider(&["provider", "deregister", "--provider-suri", "//Alice"]).is_ok());
    }

    #[test]
    fn provider_generate_key_parses() {
        let path = PathBuf::from("/tmp/provider.json");
        let (_, action) = provider(&[
            "provider",
            "generate-key",
            "--provider-keyfile",
            "/tmp/provider.json",
        ])
        .unwrap();
        assert!(matches!(
            action,
            ProviderAction::GenerateKey { provider_keyfile, words: 12, key_scheme: None }
                if provider_keyfile == path
        ));

        let (_, action) = provider(&[
            "provider",
            "generate-key",
            "--provider-keyfile",
            "/tmp/provider.json",
            "--words",
            "24",
        ])
        .unwrap();
        assert!(matches!(
            action,
            ProviderAction::GenerateKey { words: 24, .. }
        ));

        // The keyfile path is required.
        assert!(provider(&["provider", "generate-key"]).is_err());

        // Only the BIP39 word counts parse.
        for rejected in ["13", "0", "twelve"] {
            assert!(
                provider(&[
                    "provider",
                    "generate-key",
                    "--provider-keyfile",
                    "/tmp/provider.json",
                    "--words",
                    rejected
                ])
                .is_err(),
                "--words {rejected} should be rejected"
            );
        }
    }

    #[test]
    fn provider_update_settings_parses() {
        let (_, action) = provider(&[
            "provider",
            "update-settings",
            "--provider-suri",
            "//Alice",
            "--min-duration",
            "10",
            "--no-replicas",
        ])
        .unwrap();
        let ProviderAction::UpdateSettings(args) = action else {
            panic!("expected update-settings");
        };
        assert_eq!(args.settings.min_duration, Some(10));
        assert!(args.settings.no_replicas);

        // A duration above the chain's u32 block range is rejected by clap.
        assert!(provider(&[
            "provider",
            "update-settings",
            "--provider-suri",
            "//Alice",
            "--max-duration",
            "4294967296"
        ])
        .is_err());

        // Setting and clearing the replica price at once is contradictory.
        assert!(provider(&[
            "provider",
            "update-settings",
            "--provider-suri",
            "//Alice",
            "--no-replicas",
            "--replica-sync-price",
            "3"
        ])
        .is_err());
    }
}
