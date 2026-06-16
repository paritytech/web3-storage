//! `stress-test` subcommands.

use anyhow::{anyhow, bail, Context, Result};
use sp_core::crypto::Ss58Codec;
use sp_runtime::AccountId32;
use storage_client::substrate::SubstrateClient;
use storage_client::{AdminClient, ChunkingStrategy, ClientConfig, StorageUserClient};
use subxt_signer::{sr25519::Keypair, SecretUri};

use crate::cli::{GlobalArgs, UploadArgs};

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

    // Target provider: parse once, then build the hex needle the SDK reports
    // agreements with (`0x` + lowercase hex of the 32 raw account bytes).
    // Compare raw bytes, never SS58 strings (prefix differences would mismatch).
    let provider = SubstrateClient::parse_account(&args.provider)
        .map_err(|e| anyhow!("invalid --provider account: {e}"))?;
    let provider_needle = format!("0x{}", hex::encode(provider.as_ref() as &[u8]));

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
            .any(|a| a.provider.eq_ignore_ascii_case(&provider_needle))
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

/// Resolve the SURI from either `--suri` or `--keyfile` (exactly one is required;
/// clap already enforces they are mutually exclusive).
fn resolve_suri(global: &GlobalArgs) -> Result<String> {
    match (&global.suri, &global.keyfile) {
        (Some(suri), _) => Ok(suri.clone()),
        (None, Some(path)) => {
            let contents = std::fs::read_to_string(path)
                .with_context(|| format!("failed to read keyfile {}", path.display()))?;
            let suri = contents.trim().to_string();
            if suri.is_empty() {
                bail!("keyfile {} is empty", path.display());
            }
            Ok(suri)
        }
        (None, None) => bail!("one of --suri or --keyfile is required"),
    }
}
