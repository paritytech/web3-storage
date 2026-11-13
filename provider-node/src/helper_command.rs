// SPDX-License-Identifier: GPL-3.0-only

//! `storage-provider-node helper`: provider management from the command
//! line, backed by `provider-helpers` over the node's subxt chain client.

use crate::chain_connection;
use crate::cli::{key_scheme_name, ChainParams, HelperAction, HelperCommand};
use crate::subxt_client::{SubxtChainClient, GENERIC_SS58_PREFIX};
use provider_helpers::{
    Keyfile, ProviderHelper, ProviderRegistration, ProviderSettings, TxReceipt,
};
use provider_types::{KeyScheme, ProviderKeypair};
use sp_core::crypto::{AccountId32, Ss58AddressFormat, Ss58Codec};
use sp_core::{sr25519, Pair};
use tokio::sync::watch;

/// Run one helper action and exit.
///
/// Connects once, signs with the keyfile's sr25519 account (the account the
/// provider is keyed by on chain), and prints the outcome. Every error ends
/// the process with a non-zero status and the error as the message.
pub async fn run(
    chain: ChainParams,
    command: HelperCommand,
) -> Result<(), Box<dyn std::error::Error>> {
    let action = match command.action {
        HelperAction::GenerateKey { words } => {
            // Reject an unsupported scheme before connecting anywhere.
            let scheme = command.key.generate_scheme()?;
            let ss58_prefix = match connect(&chain).await {
                Ok(handle) => chain_ss58_prefix(&handle).await?,
                Err(e) => {
                    eprintln!(
                        "Cannot read the chain's SS58 prefix ({e}); the address uses prefix {GENERIC_SS58_PREFIX}"
                    );
                    GENERIC_SS58_PREFIX
                }
            };
            let output = generate_key(&command.key.keyfile, scheme, words, ss58_prefix)?;
            println!("{}", serde_json::to_string_pretty(&output)?);
            return Ok(());
        }
        action => action,
    };

    let key = command.key.load()?;
    let seed = key.seed;
    let handle = connect(&chain).await?;
    let ss58_prefix = chain_ss58_prefix(&handle).await?;

    // The chain client reads its connection from a watch channel so the node
    // can replace it on reconnect. The helper has one connection for one
    // transaction, so the sender is only kept alive for the run.
    let (_chain_tx, chain_rx) = watch::channel(Some(handle));
    let chain_client = SubxtChainClient::new(chain_rx, &seed)?.with_ss58_prefix(ss58_prefix);
    let helper = ProviderHelper::new(chain_client);
    let account = helper.account();

    match action {
        // Returned above; the chain operations follow.
        HelperAction::GenerateKey { .. } => {}
        HelperAction::Register { multiaddr, stake } => {
            let public_key = ProviderKeypair::from_seed(&seed, key.scheme)?.public_key_bytes();
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
        HelperAction::UpdateSettings(args) => {
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
        HelperAction::Deregister => {
            let receipt = helper.deregister().await?;
            println!(
                "Announced deregistration of provider {account}{}. Call complete_deregister after the announcement period to release the stake.",
                included_in(&receipt)
            );
        }
    }

    Ok(())
}

/// Open the one connection a helper run uses.
async fn connect(
    chain: &ChainParams,
) -> Result<chain_connection::ChainHandle, Box<dyn std::error::Error>> {
    let transport = chain.chain_transport()?;
    Ok(chain_connection::connect(&transport).await?)
}

/// The chain's `System::SS58Prefix` constant: the prefix every address the
/// helper prints uses, so they match the chain's own tools.
async fn chain_ss58_prefix(
    handle: &chain_connection::ChainHandle,
) -> Result<u16, Box<dyn std::error::Error>> {
    let address = storage_subxt::api::constants().system().ss58_prefix();
    Ok(handle
        .api
        .at_current_block()
        .await?
        .constants()
        .entry(address)?)
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
    path: &std::path::Path,
    scheme: KeyScheme,
    words: usize,
    ss58_prefix: u16,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let mnemonic = provider_helpers::generate_mnemonic(words)?;
    let account = AccountId32::from(
        sr25519::Pair::from_string(&mnemonic, None)
            .map_err(|e| format!("Failed to create keypair: {e:?}"))?
            .public()
            .0,
    );
    let public_key = ProviderKeypair::from_seed(&mnemonic, scheme)?.public_key_bytes();
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
    use crate::cli::HelperKeyParams;

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

    /// `generate-key` writes a JSON keyfile the node's `--keyfile` loads,
    /// with the scheme recorded, and its output names the account that file
    /// derives, encoded with the requested SS58 prefix. Any scheme other
    /// than sr25519 is rejected before anything is written.
    #[test]
    fn generate_key_writes_a_loadable_keyfile() {
        let dir = tempfile::tempdir().unwrap();
        let key = HelperKeyParams {
            keyfile: dir.path().join("provider.json"),
            key_scheme: None,
        };

        let rejected = HelperKeyParams {
            key_scheme: Some(KeyScheme::Ecdsa),
            ..key.clone()
        };
        let err = rejected.generate_scheme().unwrap_err();
        assert!(err.contains("ecdsa"), "{err}");

        let scheme = key.generate_scheme().unwrap();
        let output = generate_key(&key.keyfile, scheme, 24, 0).unwrap();
        let loaded = key.load().unwrap();
        assert_eq!(loaded.scheme, KeyScheme::Sr25519);
        let seed = loaded.seed;
        assert_eq!(seed.split(' ').count(), 24);
        let file: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&key.keyfile).unwrap()).unwrap();
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
        assert_eq!(output["keyfile"], key.keyfile.display().to_string());
        assert!(output.get("secretPhrase").is_none());

        let err = generate_key(&key.keyfile, scheme, 24, 0)
            .unwrap_err()
            .to_string();
        assert!(err.contains("exists"), "{err}");
    }
}
