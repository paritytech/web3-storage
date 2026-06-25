// SPDX-License-Identifier: Apache-2.0

//! `stress-test` subcommands.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use clap::{Args, Subcommand};
use sp_core::crypto::Ss58Codec;
use sp_runtime::AccountId32;
use storage_client::substrate::SubstrateClient;
use storage_client::{AdminClient, ClientConfig, StorageUserClient};
use subxt_signer::{sr25519::Keypair, SecretUri};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::actions::upload::{upload_once, Upload};
use crate::cli::GlobalArgs;
use crate::common::{resolve_suri, BucketId};
use crate::metrics::{summarize, OpOutcome, OpSummary};

// === Stress test subcommands ===
#[derive(Debug, Subcommand)]
pub enum StressTest {
    /// Drive configurable upload load against a provider: `users` simulated
    /// clients each performing `uploads-per-user` uploads, with either axis run
    /// sequentially or in parallel. Targets buckets the account already has an
    /// agreement with the given provider for.
    #[command(name = "upload")]
    ProviderUpload(UploadArgs),
}

// === `stress-test upload` subcommand ===
#[derive(Debug, Args)]
pub struct UploadArgs {
    /// Provider account (SS58 or 0x-hex) whose agreements select the target
    /// buckets.
    #[arg(long, value_name = "ACCOUNT")]
    pub provider: String,

    /// Cap the number of buckets written to (default: all matching buckets).
    #[arg(long, value_name = "N")]
    pub max_buckets_to_write: Option<usize>,

    /// Number of concurrent simulated users, each with its own client (1..N).
    #[arg(long, value_name = "N", default_value_t = 1)]
    pub users: usize,

    /// Number of uploads each user performs (1..X).
    #[arg(long, value_name = "X", default_value_t = 1)]
    pub uploads_per_user: usize,

    /// Exact size in bytes of each randomly-generated payload (default 0.5 MiB).
    #[arg(long, value_name = "BYTES", default_value_t = 512 * 1024)]
    pub max_payload_size: usize,

    /// Run users in parallel (default: sequential).
    #[arg(long, default_value_t = false)]
    pub parallel_users: bool,

    /// Run each user's uploads in parallel (default: sequential).
    #[arg(long, default_value_t = false)]
    pub parallel_uploads: bool,

    /// Cap total in-flight uploads across all users (0 = unbounded).
    #[arg(long, value_name = "N", default_value_t = 0)]
    pub max_concurrency: usize,
}

/// Pick the target bucket for the `global_idx`-th upload of the whole run,
/// round-robin so load spreads evenly across the selected buckets.
fn bucket_for(global_idx: usize, buckets: &[BucketId]) -> BucketId {
    buckets[global_idx % buckets.len()]
}

/// Generate a payload of exactly `size` random bytes.
fn random_payload(size: usize) -> Vec<u8> {
    use rand::RngCore;
    let mut buf = vec![0u8; size];
    rand::thread_rng().fill_bytes(&mut buf);
    buf
}

/// Perform one upload, holding a concurrency permit (if any) for its duration.
async fn do_upload(
    client: Arc<StorageUserClient>,
    bucket: BucketId,
    size: usize,
    sem: Option<Arc<Semaphore>>,
) -> OpOutcome {
    // Hold the permit until the upload completes; `acquire` only fails if the
    // semaphore is closed, which never happens here.
    let _permit = match &sem {
        Some(s) => s.acquire().await.ok(),
        None => None,
    };
    upload_once(&client, bucket, &random_payload(size)).await
}

/// Run a single user's `uploads` uploads, either sequentially or in parallel.
async fn run_user(
    user_idx: usize,
    client: Arc<StorageUserClient>,
    buckets: Arc<Vec<BucketId>>,
    uploads: usize,
    size: usize,
    parallel: bool,
    sem: Option<Arc<Semaphore>>,
) -> Vec<OpOutcome> {
    // Each user's uploads occupy a contiguous slice of the global index space so
    // the round-robin bucket assignment stays even across all users.
    let base = user_idx * uploads;
    if parallel {
        let mut set = JoinSet::new();
        for i in 0..uploads {
            let bucket = bucket_for(base + i, &buckets);
            let client = client.clone();
            let sem = sem.clone();
            set.spawn(async move { do_upload(client, bucket, size, sem).await });
        }
        let mut out = Vec::with_capacity(uploads);
        while let Some(res) = set.join_next().await {
            match res {
                Ok(outcome) => out.push(outcome),
                Err(join_err) => out.push(OpOutcome::failure(
                    size,
                    Duration::ZERO,
                    format!("upload task panicked: {join_err}"),
                )),
            }
        }
        out
    } else {
        let mut out = Vec::with_capacity(uploads);
        for i in 0..uploads {
            let bucket = bucket_for(base + i, &buckets);
            out.push(do_upload(client.clone(), bucket, size, sem.clone()).await);
        }
        out
    }
}

/// Drive configurable upload load against `--provider`.
///
/// Targets are resolved from chain (`MemberBuckets[account]` ∩ buckets with a
/// `StorageAgreements[bucket][provider]` entry); buckets and agreements are
/// never created — if nothing matches, it errors out. `--users` clients each
/// perform `--uploads-per-user` uploads of `--max-payload-size` random bytes,
/// with users and per-user uploads run sequentially or in parallel per the
/// `--parallel-*` flags, optionally capped by `--max-concurrency`.
///
/// Returns the aggregated [`OpSummary`] for the run; the caller (`main`) views
/// them. Per-upload failures are folded into the metrics, so this only returns
/// `Err` for setup failures (bad args, chain connection, no matching buckets).
pub async fn upload(global: &GlobalArgs, args: &UploadArgs) -> Result<OpSummary> {
    if args.users < 1 {
        bail!("--users must be at least 1");
    }
    if args.uploads_per_user < 1 {
        bail!("--uploads-per-user must be at least 1");
    }
    if args.max_payload_size < 1 {
        bail!("--max-payload-size must be at least 1 byte");
    }

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

    let total_uploads = args.users.saturating_mul(args.uploads_per_user);
    // Progress goes to stderr so stdout carries only the final metrics view
    // (keeping `--output json` parseable).
    eprintln!(
        "Stress test: {} user(s){}, {} upload(s)/user{}, {} bytes each, {} bucket(s) via {}{}",
        args.users,
        if args.parallel_users {
            " [parallel]"
        } else {
            " [sequential]"
        },
        args.uploads_per_user,
        if args.parallel_uploads {
            " [parallel]"
        } else {
            " [sequential]"
        },
        args.max_payload_size,
        selected_buckets_id.len(),
        global.provider_url,
        if args.max_concurrency > 0 {
            format!(", max in-flight {}", args.max_concurrency)
        } else {
            String::new()
        },
    );
    eprintln!("Running {total_uploads} upload(s)...");

    // Off-chain HTTP uploads (no chain, no signer). One client per user gives
    // each simulated user its own connection pool.
    let mut clients = Vec::with_capacity(args.users);
    for _ in 0..args.users {
        clients.push(Arc::new(
            StorageUserClient::new(config.clone())
                .context("failed to construct provider client")?,
        ));
    }

    let buckets = Arc::new(selected_buckets_id);
    let sem = (args.max_concurrency > 0).then(|| Arc::new(Semaphore::new(args.max_concurrency)));

    let started = Instant::now();
    let mut outcomes = Vec::with_capacity(total_uploads);
    // Build each user's future once; spawn it for parallelism or await it in
    // sequence. Passing the `Copy` config values positionally keeps the spawned
    // future `'static` (it owns its `usize`/`bool`/`Arc`s), so the closure only
    // borrows `buckets`/`sem`/`args` locally.
    let run_one = |user_idx: usize, client: Arc<StorageUserClient>| {
        run_user(
            user_idx,
            client,
            buckets.clone(),
            args.uploads_per_user,
            args.max_payload_size,
            args.parallel_uploads,
            sem.clone(),
        )
    };

    if args.parallel_users {
        let mut users_set = JoinSet::new();
        for (user_idx, client) in clients.into_iter().enumerate() {
            users_set.spawn(run_one(user_idx, client));
        }
        while let Some(res) = users_set.join_next().await {
            match res {
                Ok(user_outcomes) => outcomes.extend(user_outcomes),
                // A panicked user task is a bug, not load — warn and keep the
                // partial results rather than discarding the whole run.
                Err(join_err) => eprintln!("warning: a user task failed: {join_err}"),
            }
        }
    } else {
        for (user_idx, client) in clients.into_iter().enumerate() {
            outcomes.extend(run_one(user_idx, client).await);
        }
    }

    Ok(summarize(Upload, &outcomes, started.elapsed()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_payload_has_exact_size() {
        for size in [1usize, 7, 1024, 512 * 1024] {
            assert_eq!(random_payload(size).len(), size);
        }
    }

    #[test]
    fn bucket_for_round_robins() {
        let buckets = [10u64, 20, 30];
        let picked: Vec<u64> = (0..7).map(|i| bucket_for(i, &buckets)).collect();
        assert_eq!(picked, vec![10, 20, 30, 10, 20, 30, 10]);
    }
}
