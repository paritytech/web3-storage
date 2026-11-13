// SPDX-License-Identifier: Apache-2.0

//! `provider` subcommand: provider management from the command line, backed
//! by `provider-helpers` over the SDK's chain client.

use std::path::Path;

use anyhow::{anyhow, Context, Result};
use clap::{Args, Subcommand};
use provider_helpers::{
    Keyfile, ProviderHelper, ProviderRegistration, ProviderSettings, ProviderSettingsUpdate,
    TxReceipt,
};
use provider_types::{KeyScheme, ProviderKeypair};
use sp_core::crypto::{AccountId32, Ss58AddressFormat, Ss58Codec};
use sp_core::{sr25519, Pair};
use storage_client::substrate::SubstrateClient;
use storage_client::Signer;

use crate::cli::GlobalArgs;
use crate::common::{key_scheme_name, resolve_key};
use crate::provider_chain::{chain_ss58_prefix, HelperChain, GENERIC_SS58_PREFIX};

/// `storage-cli provider`: one signed provider management transaction, then
/// exit. The account is the global `--suri` or `--keyfile`.
#[derive(Debug, Args)]
pub struct ProviderCommand {
    /// Signature scheme of the signing keypair registered as the on-chain
    /// `public_key`. Only sr25519 is supported for now; any other value,
    /// from this flag or from a keyfile's `keyScheme`, is rejected.
    #[arg(long, value_enum)]
    pub key_scheme: Option<KeyScheme>,

    #[command(subcommand)]
    pub action: ProviderAction,
}

/// The provider management operations of `storage-cli provider`.
#[derive(Debug, Subcommand)]
pub enum ProviderAction {
    /// Create a new secret in `--keyfile` and print, as JSON, the account
    /// it derives. Fails if the file exists. Reads the chain's SS58 prefix
    /// from `--chain-rpc` for the address; when the chain is unreachable
    /// the generic prefix 42 is used and reported in the output.
    GenerateKey {
        /// Mnemonic length in words.
        #[arg(
            long,
            value_name = "COUNT",
            default_value_t = provider_helpers::DEFAULT_MNEMONIC_WORDS,
            value_parser = mnemonic_word_count
        )]
        words: usize,
    },

    /// Register the account as a provider.
    Register {
        /// Network address clients connect to, e.g.
        /// `/ip4/1.2.3.4/tcp/3333` or
        /// `/dns4/example.com/tcp/443/tls/http/http-path/web3-storage-provider`.
        #[arg(long, value_name = "MULTIADDR", env = "PUBLIC_MULTIADDR")]
        multiaddr: String,

        /// Stake to lock, in the chain's smallest unit. Must be at least the
        /// chain's minimum provider stake.
        #[arg(long, value_name = "AMOUNT")]
        stake: u128,
    },

    /// Change the provider's settings. Only the given flags change; every
    /// other setting keeps its on-chain value.
    UpdateSettings(SettingsUpdateArgs),

    /// Announce deregistration. The chain releases the stake once
    /// `complete_deregister` is called after the announcement period.
    Deregister,
}

/// `--words` of `provider generate-key`: one of the BIP39 word counts.
fn mnemonic_word_count(value: &str) -> Result<usize, String> {
    let counts = provider_helpers::MNEMONIC_WORD_COUNTS;
    match value.parse::<usize>() {
        Ok(words) if counts.contains(&words) => Ok(words),
        _ => Err(format!(
            "must be one of {}",
            counts.map(|c| c.to_string()).join(", ")
        )),
    }
}

/// Flags of `provider update-settings`; each one is optional.
#[derive(Debug, Default, Args)]
pub struct SettingsUpdateArgs {
    /// Shortest agreement duration, in anchor blocks, to accept. The chain
    /// counts blocks in `u32`.
    #[arg(long, value_name = "BLOCKS")]
    pub min_duration: Option<u32>,

    /// Longest agreement duration, in anchor blocks, to accept. The chain
    /// counts blocks in `u32`.
    #[arg(long, value_name = "BLOCKS")]
    pub max_duration: Option<u32>,

    /// Price per byte per anchor block, in the chain's smallest unit.
    #[arg(long, value_name = "AMOUNT")]
    pub price_per_byte: Option<u128>,

    /// Whether to accept new primary agreements.
    #[arg(long, value_name = "BOOL")]
    pub accepting_primary: Option<bool>,

    /// Price per confirmed replica sync, in the chain's smallest unit.
    /// Setting it makes the provider accept replica agreements.
    #[arg(long, value_name = "AMOUNT", conflicts_with = "no_replicas")]
    pub replica_sync_price: Option<u128>,

    /// Stop accepting replica agreements.
    #[arg(long)]
    pub no_replicas: bool,

    /// Whether to accept extensions of existing agreements.
    #[arg(long, value_name = "BOOL")]
    pub accepting_extensions: Option<bool>,

    /// Capacity limit in bytes; 0 means unlimited.
    #[arg(long, value_name = "BYTES")]
    pub max_capacity: Option<u64>,

    /// Smallest agreement size in bytes to accept; 0 means no minimum.
    #[arg(long, value_name = "BYTES")]
    pub min_agreement_bytes: Option<u64>,
}

impl SettingsUpdateArgs {
    /// The partial update these flags describe.
    pub fn update(&self) -> ProviderSettingsUpdate {
        let replica_sync_price = if self.no_replicas {
            Some(None)
        } else {
            self.replica_sync_price.map(Some)
        };
        ProviderSettingsUpdate {
            min_duration: self.min_duration.map(u64::from),
            max_duration: self.max_duration.map(u64::from),
            price_per_byte: self.price_per_byte,
            accepting_primary: self.accepting_primary,
            replica_sync_price,
            accepting_extensions: self.accepting_extensions,
            max_capacity: self.max_capacity,
            min_agreement_bytes: self.min_agreement_bytes,
        }
    }
}

/// Run one provider action.
///
/// Connects once, signs with the account's sr25519 keypair (the account the
/// provider is keyed by on chain), and prints the outcome.
pub async fn run(global: &GlobalArgs, command: ProviderCommand) -> Result<()> {
    let action = match command.action {
        ProviderAction::GenerateKey { words } => {
            let path = global.keyfile.as_deref().ok_or_else(|| {
                anyhow!("generate-key needs --keyfile: the path of the keyfile to create")
            })?;
            // Reject an unsupported scheme before connecting anywhere.
            let scheme = helper_scheme(command.key_scheme.unwrap_or_default())?;
            let ss58_prefix = match SubstrateClient::connect(&global.chain_rpc).await {
                Ok(client) => chain_ss58_prefix(&client).await?,
                Err(e) => {
                    eprintln!(
                        "Cannot read the chain's SS58 prefix ({e}); the address uses prefix {GENERIC_SS58_PREFIX}"
                    );
                    GENERIC_SS58_PREFIX
                }
            };
            let output = generate_key(path, scheme, words, ss58_prefix)?;
            println!("{}", serde_json::to_string_pretty(&output)?);
            return Ok(());
        }
        action => action,
    };

    let key = resolve_key(global, command.key_scheme)?;
    helper_scheme(key.scheme)?;
    let signer = Signer::from_seed(&key.seed).context("failed to derive keypair from SURI")?;
    let client = SubstrateClient::connect(&global.chain_rpc)
        .await?
        .with_signer(signer);
    let ss58_prefix = chain_ss58_prefix(&client).await?;
    let helper = ProviderHelper::new(HelperChain::new(client, ss58_prefix)?);
    let account = helper.account();

    match action {
        // Returned above; the chain operations follow.
        ProviderAction::GenerateKey { .. } => {}
        ProviderAction::Register { multiaddr, stake } => {
            let public_key = ProviderKeypair::from_seed(&key.seed, key.scheme)
                .map_err(anyhow::Error::msg)?
                .public_key_bytes();
            let registration = ProviderRegistration {
                multiaddr,
                public_key,
                stake,
            };
            let receipt = helper.register(&registration).await?;
            println!(
                "Registered provider {account} at {} with stake {}{}",
                registration.multiaddr,
                registration.stake,
                included_in(&receipt)
            );
        }
        ProviderAction::UpdateSettings(args) => {
            let outcome = helper.update_settings(&args.update()).await?;
            match outcome.receipt {
                Some(receipt) => println!(
                    "Updated settings of provider {account}{}",
                    included_in(&receipt)
                ),
                None => println!("Settings of provider {account} already match; nothing submitted"),
            }
            print_settings(&outcome.settings);
        }
        ProviderAction::Deregister => {
            let receipt = helper.deregister().await?;
            println!(
                "Announced deregistration of provider {account}{}. Call complete_deregister after the announcement period to release the stake.",
                included_in(&receipt)
            );
        }
    }

    Ok(())
}

/// The subcommand supports sr25519 only for now.
fn helper_scheme(scheme: KeyScheme) -> Result<KeyScheme> {
    match scheme {
        KeyScheme::Sr25519 => Ok(scheme),
        other => Err(anyhow!(
            "key scheme {} is not supported by storage-cli provider yet; only sr25519 is",
            key_scheme_name(other)
        )),
    }
}

/// Create the keyfile and return its public part for printing: the keyfile
/// without `secretPhrase`, plus the path. The secret is only in the file,
/// which is the copy to back up; a terminal is often logged.
///
/// The keyfile records, in the field names `subkey generate --output-type
/// json` uses, the sr25519 submission account the provider is keyed by on
/// chain (`ss58Address`, `accountId`) and the key that `register` puts on
/// chain (`keyScheme`, `publicKey`).
fn generate_key(
    path: &Path,
    scheme: KeyScheme,
    words: usize,
    ss58_prefix: u16,
) -> Result<serde_json::Value> {
    let mnemonic = provider_helpers::generate_mnemonic(words)?;
    let account = AccountId32::from(
        sr25519::Pair::from_string(&mnemonic, None)
            .map_err(|e| anyhow!("Failed to create keypair: {e:?}"))?
            .public()
            .0,
    );
    let public_key = ProviderKeypair::from_seed(&mnemonic, scheme)
        .map_err(anyhow::Error::msg)?
        .public_key_bytes();
    let keyfile = Keyfile {
        secret_phrase: mnemonic,
        key_scheme: Some(key_scheme_name(scheme)),
        identity: [
            ("ss58Prefix", serde_json::Value::from(ss58_prefix)),
            (
                "ss58Address",
                account
                    .to_ss58check_with_version(Ss58AddressFormat::custom(ss58_prefix))
                    .into(),
            ),
            (
                "accountId",
                array_bytes::bytes2hex("0x", account.as_ref() as &[u8]).into(),
            ),
            ("publicKey", array_bytes::bytes2hex("0x", public_key).into()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect(),
    };
    provider_helpers::write_keyfile(path, &keyfile)?;

    let mut public = serde_json::Map::new();
    public.insert("keyfile".into(), path.display().to_string().into());
    public.insert("keyScheme".into(), key_scheme_name(scheme).into());
    public.extend(keyfile.identity);
    Ok(serde_json::Value::Object(public))
}

/// One `name: value` line per setting, in the pallet's field order.
fn print_settings(settings: &ProviderSettings) {
    let replica_sync_price = match settings.replica_sync_price {
        Some(price) => price.to_string(),
        None => "none (not accepting replicas)".to_string(),
    };
    println!("  min_duration: {}", settings.min_duration);
    println!("  max_duration: {}", settings.max_duration);
    println!("  price_per_byte: {}", settings.price_per_byte);
    println!("  accepting_primary: {}", settings.accepting_primary);
    println!("  replica_sync_price: {replica_sync_price}");
    println!("  accepting_extensions: {}", settings.accepting_extensions);
    println!("  max_capacity: {}", settings.max_capacity);
    println!("  min_agreement_bytes: {}", settings.min_agreement_bytes);
}

/// The block that included the transaction, for the outcome line.
fn included_in(receipt: &TxReceipt) -> String {
    match &receipt.block_hash {
        Some(hash) => format!(" (finalized in block {hash})"),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::tests::global_args;

    /// A generated mnemonic is a secret every signing scheme and the
    /// submission account derive from: the BIP39 phrase `provider-helpers`
    /// produces is the format `sp_core` parses.
    #[test]
    fn generated_mnemonic_derives_under_every_scheme() {
        let mnemonic = provider_helpers::generate_mnemonic(12).unwrap();
        sr25519::Pair::from_string(&mnemonic, None).unwrap();
        for scheme in [
            KeyScheme::Sr25519,
            KeyScheme::Ed25519,
            KeyScheme::Ecdsa,
            KeyScheme::Eth,
        ] {
            let key = ProviderKeypair::from_seed(&mnemonic, scheme)
                .unwrap()
                .public_key_bytes();
            assert!(
                key.len() == 32 || key.len() == 33,
                "{scheme:?}: {}",
                key.len()
            );
        }
    }

    /// `generate-key` writes a JSON keyfile that `--keyfile` loads, with the
    /// scheme recorded, and its output names the account that file derives,
    /// encoded with the requested SS58 prefix. Any scheme other than sr25519
    /// is rejected before anything is written.
    #[test]
    fn generate_key_writes_a_loadable_keyfile() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("provider.json");

        let err = helper_scheme(KeyScheme::Ecdsa).unwrap_err().to_string();
        assert!(err.contains("ecdsa") && err.contains("sr25519"), "{err}");

        let scheme = helper_scheme(KeyScheme::default()).unwrap();
        let output = generate_key(&path, scheme, 24, 0).unwrap();
        let loaded = resolve_key(&global_args(None, Some(path.clone())), None).unwrap();
        assert_eq!(loaded.scheme, KeyScheme::Sr25519);
        let seed = loaded.seed;
        assert_eq!(seed.split(' ').count(), 24);
        let file: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(file["secretPhrase"], seed);
        assert_eq!(file["keyScheme"], "sr25519");
        assert_eq!(file["ss58Address"], output["ss58Address"]);

        let expected =
            AccountId32::from(sr25519::Pair::from_string(&seed, None).unwrap().public().0);
        let (decoded, format) =
            AccountId32::from_ss58check_with_version(output["ss58Address"].as_str().unwrap())
                .unwrap();
        assert_eq!(decoded, expected);
        assert_eq!(format, Ss58AddressFormat::custom(0));
        assert_eq!(output["ss58Prefix"], 0);
        assert_eq!(
            output["accountId"],
            array_bytes::bytes2hex("0x", expected.as_ref() as &[u8])
        );
        assert_eq!(output["keyScheme"], "sr25519");
        assert_eq!(output["publicKey"], output["accountId"]);
        assert_eq!(output["keyfile"], path.display().to_string());
        assert!(output.get("secretPhrase").is_none());

        let err = generate_key(&path, scheme, 24, 0).unwrap_err().to_string();
        assert!(err.contains("exists"), "{err}");
    }

    #[test]
    fn update_settings_flags_map_to_a_partial_update() {
        assert_eq!(
            SettingsUpdateArgs::default().update(),
            ProviderSettingsUpdate::default()
        );

        let args = SettingsUpdateArgs {
            min_duration: Some(10),
            price_per_byte: Some(7),
            accepting_primary: Some(false),
            replica_sync_price: Some(3),
            max_capacity: Some(1024),
            ..Default::default()
        };
        assert_eq!(
            args.update(),
            ProviderSettingsUpdate {
                min_duration: Some(10),
                price_per_byte: Some(7),
                accepting_primary: Some(false),
                replica_sync_price: Some(Some(3)),
                max_capacity: Some(1024),
                ..Default::default()
            }
        );

        let args = SettingsUpdateArgs {
            no_replicas: true,
            ..Default::default()
        };
        assert_eq!(args.update().replica_sync_price, Some(None));
    }
}
