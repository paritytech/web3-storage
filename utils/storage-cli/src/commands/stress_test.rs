// SPDX-License-Identifier: Apache-2.0

//! `stress-test` subcommands.

use anyhow::{anyhow, bail, Context, Result};
use clap::{Args, Subcommand};
use sp_core::crypto::Ss58Codec;
use sp_runtime::AccountId32;
use storage_client::substrate::SubstrateClient;
use storage_client::{AdminClient, ChunkingStrategy, ClientConfig, StorageUserClient};
use subxt_signer::{sr25519::Keypair, SecretUri};

use crate::cli::GlobalArgs;
use crate::common::resolve_suri;

// === Stress test subcommands ===
#[derive(Debug, Subcommand)]
pub enum StressTest {
    /// Upload generated data to every bucket the account already has an
    /// agreement with the given provider for.
    #[command(name = "upload")]
    ProviderUpload(UploadArgs),
}

// === `stress-test provider-upload` subcommand ===
#[derive(Debug, Args)]
pub struct UploadArgs {
    /// Provider account (SS58 or 0x-hex) whose agreements select the target
    /// buckets.
    #[arg(long, value_name = "ACCOUNT")]
    pub provider: String,

    /// Cap the number of buckets written to (default: all matching buckets).
    #[arg(long, value_name = "N")]
    pub max_buckets_to_write: Option<usize>,

    /// Bytes of generated data to upload per bucket.
    #[arg(long, value_name = "BYTES", default_value_t = 1024 * 1024)]
    pub size: usize,
}

/// Upload generated data to every bucket the account already has an agreement
/// with `--provider` for.
///
/// This resolves targets from chain (`MemberBuckets[account]` ∩ buckets with a
/// `StorageAgreements[bucket][provider]` entry) and never creates buckets or
/// agreements — if nothing matches, it errors out.
pub async fn upload(global: &GlobalArgs, args: &UploadArgs) -> Result<()> {
    // Identity: derive the account whose buckets we look up from the SURI.
    let suri = resolve_suri(global)?;
    let keypair = Keypair::from_uri(&suri.parse::<SecretUri>().context("failed to parse SURI")?)
        .context("failed to derive keypair from SURI")?;
    let account = AccountId32::from(keypair.public_key().0);
    let account_ss58 = account.to_ss58check();

    // Target provider: parse the input and hex-encode it (`0x` + lowercase hex
    // of the 32 raw account bytes) for matching against the chain's
    // `StorageAgreements`. Match on raw bytes, never SS58 strings — prefix
    // differences (`5…` vs `1…`) would make equal accounts compare unequal.
    let target_provider_hex = SubstrateClient::parse_account(&args.provider)
        .map_err(|e: storage_client::ClientError| anyhow!("invalid --provider account: {e}"))
        .map(|ac| format!("0x{}", hex::encode(ac.as_ref() as &[u8])))?;

    let config = ClientConfig {
        chain_ws_url: global.chain_rpc.clone(),
        provider_urls: vec![global.provider_url.clone()],
        ..Default::default()
    };

    // Read-only chain access: resolve the buckets that have an agreement with
    // the target provider. No signer is set — uploads are off-chain HTTP.
    let mut admin = AdminClient::new(config.clone(), account_ss58.clone())
        .context("failed to construct chain client")?;
    admin
        .connect()
        .await
        .with_context(|| format!("failed to connect to chain RPC {}", global.chain_rpc))?;

    let all_buckets_id = admin
        .list_my_buckets()
        .await
        .context("failed to read the account's buckets from chain")?;

    let mut selected_buckets_id = Vec::new();
    for bucket_id in all_buckets_id {
        let agreements = admin
            .list_bucket_agreements(bucket_id)
            .await
            .with_context(|| format!("failed to read agreements for bucket {bucket_id}"))?;
        if agreements
            .iter()
            .any(|a| a.provider.eq_ignore_ascii_case(&target_provider_hex))
        {
            selected_buckets_id.push(bucket_id);
        }
    }

    if selected_buckets_id.is_empty() {
        bail!(
            "account {account_ss58} has no buckets with an agreement to provider {} on {}. \
             Nothing to upload (no bucket or agreement was created).",
            args.provider,
            global.chain_rpc,
        );
    }

    if let Some(max) = args.max_buckets_to_write {
        selected_buckets_id.truncate(max);
    }

    println!(
        "Uploading {} bytes to {} bucket(s) via {}",
        args.size,
        selected_buckets_id.len(),
        global.provider_url,
    );

    // Off-chain HTTP uploads (no chain, no signer). Constant-fill payload,.
    let user = StorageUserClient::new(config).context("failed to construct provider client")?;
    let payload = vec![0x42; args.size];

    for bucket in &selected_buckets_id {
        let data_root = user
            .upload(*bucket, &payload, ChunkingStrategy::default())
            .await
            .with_context(|| format!("upload to bucket {bucket} failed"))?;
        println!(
            "  bucket {bucket}: uploaded {} bytes, data_root 0x{}",
            payload.len(),
            hex::encode(data_root.as_bytes()),
        );
    }

    println!("Done: {} bucket(s) written.", selected_buckets_id.len());
    Ok(())
}
