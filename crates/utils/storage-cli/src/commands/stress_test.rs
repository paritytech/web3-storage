// SPDX-License-Identifier: Apache-2.0

//! `stress-test` subcommands.

use std::num::{NonZeroU32, NonZeroU64, NonZeroUsize};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use clap::{Args, Subcommand};
use sp_runtime::AccountId32;
use storage_client::substrate::SubstrateClient;
use storage_client::{AdminClient, ClientConfig, Signer, StorageUserClient};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::actions::upload::{upload_once, Upload};
use crate::cli::GlobalArgs;
use crate::common::{account_ss58, build_config, resolve_signer, BucketId};
use crate::metrics::{summarize, OpOutcome, OpSummary};
use crate::prepare::{self, BucketSource, PreparationReport, PrepareParams};

// === Stress test subcommands ===
#[derive(Debug, Subcommand)]
pub enum StressTest {
    /// Drive configurable upload load against a provider: `users` simulated
    /// clients each performing `uploads-per-user` uploads, with either axis run
    /// sequentially or in parallel. Targets buckets the account already has an
    /// agreement with the given provider for, or buckets the run prepares
    /// itself (`--prepare-buckets`).
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

    /// Cap the number of buckets written to (1..N; default: all matching buckets).
    #[arg(long, value_name = "N")]
    pub max_buckets_to_write: Option<NonZeroUsize>,

    /// Number of concurrent simulated users, each with its own client (1..N).
    #[arg(long, value_name = "N", default_value = "1")]
    pub users: NonZeroUsize,

    /// Number of uploads each user performs (1..X).
    #[arg(long, value_name = "X", default_value = "1")]
    pub uploads_per_user: NonZeroUsize,

    /// Exact size in bytes of each randomly-generated payload (default 0.5 MiB).
    #[arg(long, value_name = "BYTES", default_value = "524288")]
    pub payload_size: NonZeroUsize,

    /// Run users in parallel (default: sequential).
    #[arg(long, default_value_t = false)]
    pub parallel_users: bool,

    /// Run each user's uploads in parallel (default: sequential).
    #[arg(long, default_value_t = false)]
    pub parallel_uploads: bool,

    /// Cap total in-flight uploads across all users (0 = unbounded).
    #[arg(long, value_name = "N", default_value_t = 0)]
    pub max_concurrency: usize,

    /// Prepare N fresh buckets, each with a primary agreement to `--provider`,
    /// before the run. Setup only: excluded from the measured window. 0 = off.
    #[arg(long, value_name = "N", default_value_t = 0)]
    pub prepare_buckets: usize,

    /// Quota negotiated per prepared bucket. Default: this run's per-bucket
    /// load plus 10% headroom.
    #[arg(long, value_name = "BYTES")]
    pub prepare_max_bytes: Option<NonZeroU64>,

    /// Agreement duration for prepared buckets, in anchor blocks.
    #[arg(long, value_name = "BLOCKS", default_value = "100")]
    pub prepare_duration: NonZeroU32,

    /// Price per byte per block the run accepts for prepared buckets. The
    /// provider rejects offers below its listed price and signs at that price.
    #[arg(long, value_name = "P", default_value_t = 1)]
    pub prepare_price_per_byte: u128,

    /// Where target buckets come from. Default: `create` when
    /// `--prepare-buckets` is set, `discover` otherwise.
    #[arg(long, value_enum, value_name = "MODE")]
    pub bucket_source: Option<BucketSource>,
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

/// Resolve the buckets the admin's account can upload to via `provider_hex`:
/// those with a `StorageAgreements[bucket][provider]` entry. Read-only; empty
/// when nothing matches.
async fn discover_target_buckets(admin: &AdminClient, provider_hex: &str) -> Result<Vec<BucketId>> {
    let all_buckets_id = admin
        .list_my_buckets()
        .await
        .context("failed to read the account's buckets from chain")?;

    let mut selected = Vec::new();
    for bucket_id in all_buckets_id {
        let agreements = admin
            .list_bucket_agreements(bucket_id)
            .await
            .with_context(|| format!("failed to read agreements for bucket {bucket_id}"))?;
        if agreements
            .iter()
            .any(|a| a.provider.eq_ignore_ascii_case(provider_hex))
        {
            selected.push(bucket_id);
        }
    }

    Ok(selected)
}

/// Separate clients so each user drives its own connection pool, and signs its
/// own provider `Authorization` headers.
fn build_clients_per_user(
    config: &ClientConfig,
    signer: &Signer,
    count: usize,
) -> Result<Vec<Arc<StorageUserClient>>> {
    (0..count)
        .map(|_| {
            StorageUserClient::new(config.clone(), signer.clone())
                .map(Arc::new)
                .context("failed to construct provider client")
        })
        .collect()
}

/// Label for a parallelism axis in the progress banner.
fn axis(parallel: bool) -> &'static str {
    if parallel {
        "[parallel]"
    } else {
        "[sequential]"
    }
}

/// Print the run configuration to stderr (stdout carries only the final metrics
/// view, keeping `--output json` parseable).
fn print_banner(args: &UploadArgs, total_uploads: usize, bucket_count: usize, provider_url: &str) {
    let cap = if args.max_concurrency > 0 {
        format!(", max in-flight {}", args.max_concurrency)
    } else {
        String::new()
    };
    eprintln!(
        "Stress test: {} user(s) {}, {} upload(s)/user {}, {} bytes each, {} bucket(s) via {}{}",
        args.users,
        axis(args.parallel_users),
        args.uploads_per_user,
        axis(args.parallel_uploads),
        args.payload_size,
        bucket_count,
        provider_url,
        cap,
    );
    eprintln!("Running {total_uploads} upload(s)...");
}

/// Drive the configured upload load: one future per user, spawned for
/// parallelism or awaited in sequence, collecting every [`OpOutcome`].
async fn run_load(
    clients: Vec<Arc<StorageUserClient>>,
    buckets: Arc<Vec<BucketId>>,
    args: &UploadArgs,
) -> Vec<OpOutcome> {
    let sem = (args.max_concurrency > 0).then(|| Arc::new(Semaphore::new(args.max_concurrency)));
    let mut outcomes = Vec::with_capacity(clients.len() * args.uploads_per_user.get());

    // Build each user's future once; spawn it for parallelism or await it in
    // sequence.
    let run_one = |user_idx: usize, client: Arc<StorageUserClient>| {
        run_user(
            user_idx,
            client,
            buckets.clone(),
            args.uploads_per_user.get(),
            args.payload_size.get(),
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
    outcomes
}

/// Drive configurable upload load against `--provider`.
///
/// Target buckets are discovered on chain, prepared by this run, or both, per
/// `--bucket-source`. Per-upload failures are folded into the metrics, so
/// `Err` means a setup failure: bad provider, chain connection, no target
/// buckets, or preparation.
pub async fn upload(
    global: &GlobalArgs,
    args: &UploadArgs,
) -> Result<(OpSummary, Option<PreparationReport>)> {
    let signer = resolve_signer(global)?;
    let account_ss58 = account_ss58(&signer);

    let provider_hex = SubstrateClient::parse_account(&args.provider)
        .map_err(|e: storage_client::ClientError| anyhow!("invalid --provider account: {e}"))
        .map(|ac| format!("0x{}", hex::encode(ac.as_ref() as &[u8])))?;

    let config = build_config(global);

    prepare::check_prepare_flags(args.prepare_buckets, args.prepare_max_bytes)?;
    let source = BucketSource::resolve(args.bucket_source, args.prepare_buckets)?;

    // One chain connection serves both discovery and preparation.
    let mut admin = AdminClient::new(config.clone(), signer.clone())
        .context("failed to construct chain client")?;
    admin
        .connect()
        .await
        .with_context(|| format!("failed to connect to chain RPC {}", global.chain_rpc))?;

    let discovered = if source == BucketSource::Create {
        Vec::new()
    } else {
        discover_target_buckets(&admin, &provider_hex).await?
    };
    let plan = prepare::plan_targets(
        source,
        args.prepare_buckets,
        discovered,
        args.max_buckets_to_write,
    )?;

    let total_uploads = args.users.get().saturating_mul(args.uploads_per_user.get());
    let mut buckets = plan.existing;
    let mut preparation = None;
    if plan.to_prepare > 0 {
        let bucket_count = buckets.len().saturating_add(plan.to_prepare);
        let needed =
            prepare::bytes_per_bucket(total_uploads, args.payload_size.get(), bucket_count)?;
        let max_bytes = prepare::resolve_max_bytes(args.prepare_max_bytes, needed)?;
        let report = prepare::prepare_buckets(
            &admin,
            PrepareParams {
                owner: AccountId32::from(signer.keypair().public_key().0),
                provider: &args.provider,
                provider_url: &global.provider_url,
                count: plan.to_prepare,
                max_bytes,
                duration: args.prepare_duration.get(),
                price_per_byte: args.prepare_price_per_byte,
            },
        )
        .await?;
        buckets.extend_from_slice(&report.buckets);
        preparation = Some(report);
    }

    if buckets.is_empty() {
        bail!(
            "account {account_ss58} has no buckets with an agreement to provider {} on {}. \
             Nothing to upload (no bucket or agreement was created); pass --prepare-buckets N \
             to open some.",
            args.provider,
            global.chain_rpc
        );
    }

    print_banner(args, total_uploads, buckets.len(), &global.provider_url);

    let clients = build_clients_per_user(&config, &signer, args.users.get())?;
    // The clock starts after preparation, so the numbers cover uploads only.
    let started = Instant::now();
    let outcomes = run_load(clients, Arc::new(buckets), args).await;
    Ok((summarize(Upload, &outcomes, started.elapsed()), preparation))
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
    fn zero_valued_count_flags_are_rejected() {
        use clap::Parser;

        #[derive(Debug, Parser)]
        struct Wrapper {
            #[clap(flatten)]
            args: UploadArgs,
        }

        for flag in [
            "--max-buckets-to-write",
            "--users",
            "--uploads-per-user",
            "--payload-size",
            "--prepare-max-bytes",
            "--prepare-duration",
        ] {
            assert!(
                Wrapper::try_parse_from(["stress-test-upload", "--provider", "//Alice", flag, "0"])
                    .is_err(),
                "{flag} 0 should be rejected"
            );
        }
    }

    #[test]
    fn prepare_flags_parse_with_defaults_off() {
        use clap::Parser;

        #[derive(Debug, Parser)]
        struct Wrapper {
            #[clap(flatten)]
            args: UploadArgs,
        }

        let w = Wrapper::try_parse_from(["stress-test-upload", "--provider", "//Alice"]).unwrap();
        assert_eq!(w.args.prepare_buckets, 0);
        assert!(w.args.prepare_max_bytes.is_none());
        assert_eq!(w.args.prepare_duration.get(), 100);
        assert_eq!(w.args.prepare_price_per_byte, 1);
        assert!(w.args.bucket_source.is_none());

        let w = Wrapper::try_parse_from([
            "stress-test-upload",
            "--provider",
            "//Alice",
            "--prepare-buckets",
            "2",
            "--bucket-source",
            "auto",
        ])
        .unwrap();
        assert_eq!(w.args.prepare_buckets, 2);
        assert_eq!(w.args.bucket_source, Some(BucketSource::Auto));
    }

    #[test]
    fn bucket_for_round_robins() {
        let buckets = [10u64, 20, 30];
        let picked: Vec<u64> = (0..7).map(|i| bucket_for(i, &buckets)).collect();
        assert_eq!(picked, vec![10, 20, 30, 10, 20, 30, 10]);
    }
}
