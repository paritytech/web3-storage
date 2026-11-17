// SPDX-License-Identifier: Apache-2.0

//! `provider` subcommands.

use anyhow::{anyhow, bail, Context, Result};
use clap::{Args, Subcommand};
use provider_types::{KeyScheme, ProviderKeypair};
use sp_runtime::AccountId32;
use storage_client::{ProviderClient, ProviderSettings};

use crate::cli::GlobalArgs;
use crate::common::{account_ss58, build_config, signer_from_suri, ProviderKey};

/// Stake locked at registration: `MinProviderStake` of the bundled runtimes
/// (1000 tokens at 12 decimals).
const STAKE: u128 = 1_000_000_000_000_000;

#[derive(Debug, Subcommand)]
pub enum Provider {
    /// Register the provider account as a storage provider that accepts
    /// primary agreements at 1 per byte-block. Does nothing if the account is
    /// already registered with the same signing key. The provider node can start
    /// before or after this command.
    Register(RegisterArgs),
}

#[derive(Debug, Args)]
pub struct RegisterArgs {
    #[command(flatten)]
    pub provider: ProviderKey,

    /// Multiaddr at which clients reach the provider node.
    #[arg(long, value_name = "MULTIADDR")]
    pub multiaddr: String,

    /// Scheme of the signing key to register. Must match the provider node's
    /// `--key-scheme`. The extrinsics are always signed by the sr25519
    /// account.
    #[arg(long, value_enum, value_name = "SCHEME", default_value_t = KeyScheme::Sr25519)]
    pub key_scheme: KeyScheme,
}

/// Register the `--provider-suri`/`--provider-keyfile` account as a provider
/// and open it for primary agreements.
pub async fn register(global: &GlobalArgs, args: &RegisterArgs) -> Result<()> {
    let suri = args.provider.suri()?;
    let signer = signer_from_suri(&suri)?;
    let provider_ss58 = account_ss58(&signer);
    let provider_account = AccountId32::from(signer.keypair().public_key().0);
    let signing_key = ProviderKeypair::from_seed(&suri, args.key_scheme)
        .map_err(|error| {
            anyhow!(
                "failed to derive the {:?} signing key: {error}",
                args.key_scheme
            )
        })?
        .public_key_bytes();

    let mut client = ProviderClient::new(build_config(global), signer)
        .context("failed to construct chain client")?;
    client
        .connect()
        .await
        .with_context(|| format!("failed to connect to chain RPC {}", global.chain_rpc))?;

    let registration = client
        .get_provider_info(&provider_account)
        .await
        .with_context(|| format!("failed to read the registration of {provider_ss58}"))?;
    if let Some(registration) = registration {
        if registration.public_key != signing_key {
            bail!(
                "provider {provider_ss58} is already registered with key {}, not with the {:?} \
                 key {} derived from this seed. Deregister first, or pass the seed and \
                 --key-scheme that produced the registered key.",
                array_bytes::bytes2hex("0x", &registration.public_key),
                args.key_scheme,
                array_bytes::bytes2hex("0x", &signing_key),
            );
        }
        eprintln!("Provider {provider_ss58} is already registered with this key");
        return Ok(());
    }

    eprintln!("Registering provider {provider_ss58} at {}", args.multiaddr);
    client
        .register(args.multiaddr.clone(), signing_key, STAKE)
        .await
        .with_context(|| format!("failed to register provider {provider_ss58}"))?;
    client
        .update_settings(ProviderSettings {
            price_per_byte: 1,
            min_duration: 10,
            max_duration: 100_000,
            accepting_primary: true,
            replica_sync_price: None,
            accepting_extensions: true,
            max_capacity: 0,
            min_agreement_bytes: 0,
        })
        .await
        .with_context(|| format!("failed to update the settings of provider {provider_ss58}"))?;
    eprintln!("Provider {provider_ss58} registered and accepting primary agreements");
    Ok(())
}
