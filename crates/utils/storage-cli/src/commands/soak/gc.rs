// SPDX-License-Identifier: Apache-2.0

//! `soak gc`: drive the provider's garbage collector through repeated prune
//! cycles and bucket teardowns.

use std::num::{NonZeroU32, NonZeroU64};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, ensure, Context, Result};
use clap::Args;
use sp_core::H256;
use sp_runtime::AccountId32;
use storage_client::substrate::SubstrateClient;
use storage_client::{
    AdminClient, ChunkingStrategy, ClientError, Commitment, ProviderClient, StorageUserClient,
};
use storage_primitives::DEFAULT_CHUNK_SIZE;

use super::LaneKeys;
use crate::cli::GlobalArgs;
use crate::common::{build_config, signer_from_suri, BucketId};
use crate::prepare::{self, PrepareParams};
use crate::soak::{parse_duration, poll_until, CycleRecord, SoakFailure, SoakReport, StopSignal};

/// Wait for the provider to read the quota from the chain agreement.
const QUOTA_SYNC_TIMEOUT: Duration = Duration::from_secs(90);

/// Pause between provider reads while waiting for a condition.
const POLL_STEP: Duration = Duration::from_secs(1);

/// Run time when neither `--duration` nor `--epochs` is given.
const DEFAULT_DURATION: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, Args)]
pub struct GcArgs {
    #[command(flatten)]
    pub lanes: LaneKeys,

    /// Public account of the provider (SS58 or 0x-hex) the buckets are opened with.
    #[arg(long, value_name = "ACCOUNT")]
    pub provider_account: String,

    /// Prune cycles a bucket runs before its agreement ends.
    #[arg(long, value_name = "N", default_value = "5")]
    pub cycles_per_bucket: NonZeroU32,

    /// Chunks uploaded per cycle (at least 2: one is pruned, one remains).
    #[arg(long, value_name = "N", default_value = "3", value_parser = clap::value_parser!(u32).range(2..))]
    pub chunks_per_cycle: u32,

    /// Bytes per chunk, from 64 up to the chunk size limit.
    #[arg(long, value_name = "BYTES", default_value = "100",
          value_parser = clap::value_parser!(u32).range(64..i64::from(DEFAULT_CHUNK_SIZE)))]
    pub chunk_bytes: u32,

    /// Stop starting epochs after this long: `90s`, `30m` or `4h`. Default 30m unless `--epochs` is set.
    #[arg(long, value_name = "DURATION", value_parser = parse_duration, conflicts_with = "epochs")]
    pub duration: Option<Duration>,

    /// Stop after this many epochs.
    #[arg(long, value_name = "N")]
    pub epochs: Option<NonZeroU32>,

    /// Agreement length of each bucket, in anchor blocks.
    #[arg(long, value_name = "BLOCKS", default_value = "600")]
    pub agreement_duration: NonZeroU32,

    /// The provider's `GC_SCAN_INTERVAL` in seconds. Sets how long a drain may take.
    #[arg(long, value_name = "SECS", default_value = "30")]
    pub gc_scan_interval: NonZeroU64,

    /// Longest wait for one erasure, in seconds.
    #[arg(long, value_name = "SECS", default_value = "120")]
    pub erase_timeout: NonZeroU64,

    /// Provider log file. A "GC reconcile skipped" line fails the run.
    #[arg(long, value_name = "PATH")]
    pub provider_log: Option<PathBuf>,
}

/// When a run stops starting epochs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopAt {
    Deadline(Duration),
    Epochs(u32),
}

/// Run settings derived from [`GcArgs`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GcPlan {
    pub chunk_bytes: u64,
    pub chunks_per_cycle: u32,
    pub cycles_per_bucket: u32,
    /// Bucket quota: the chunks of one cycle plus the survivor of the last.
    pub max_bytes: u64,
    pub stop: StopAt,
    pub erase_timeout: Duration,
    /// Longest wait for the provider to empty: two scan passes plus a minute.
    pub drain_timeout: Duration,
}

impl GcArgs {
    pub fn plan(&self) -> GcPlan {
        let chunk_bytes = u64::from(self.chunk_bytes);
        let stop = match (self.epochs, self.duration) {
            (Some(epochs), _) => StopAt::Epochs(epochs.get()),
            (None, Some(duration)) => StopAt::Deadline(duration),
            (None, None) => StopAt::Deadline(DEFAULT_DURATION),
        };
        GcPlan {
            chunk_bytes,
            chunks_per_cycle: self.chunks_per_cycle,
            cycles_per_bucket: self.cycles_per_bucket.get(),
            max_bytes: (u64::from(self.chunks_per_cycle) + 1) * chunk_bytes,
            stop,
            erase_timeout: Duration::from_secs(self.erase_timeout.get()),
            drain_timeout: Duration::from_secs(2 * self.gc_scan_interval.get() + 60),
        }
    }
}

/// A chunk payload of `size` bytes, unique per run, lane, epoch, cycle and
/// index. Nodes are shared across buckets by hash, so equal payloads would share storage.
pub fn chunk_payload(
    run_id: &str,
    lane: usize,
    epoch: u32,
    cycle: u32,
    index: u32,
    size: usize,
) -> Vec<u8> {
    let mut payload = format!("{run_id}-{lane}.{epoch}.{cycle}.{index}-").into_bytes();
    if payload.len() < size {
        payload.resize(size, b'x');
    }
    payload
}

/// Settings and clients shared by all lanes of a run.
struct GcContext {
    plan: GcPlan,
    provider_url: String,
    provider_account: String,
    agreement_duration: u32,
    price_per_byte: u128,
    run_id: String,
    stop: StopSignal,
}

/// One client account with its own bucket.
struct Lane {
    index: usize,
    account: AccountId32,
    user: StorageUserClient,
    admin: AdminClient,
    bucket: Option<BucketId>,
    /// Bytes the provider must report as used on the current bucket.
    live_bytes: u64,
    /// Newest chunk of the previous cycle: still live, pruned by the next cycle.
    survivor: Option<H256>,
}

/// Run the settings checks, then one prune cycle on a new bucket of each lane.
pub async fn run(global: &GlobalArgs, args: &GcArgs) -> Result<SoakReport> {
    let suris = args.lanes.suris()?;
    let plan = args.plan();
    let provider_account = SubstrateClient::parse_account(&args.provider_account)
        .map_err(|e| anyhow!("invalid --provider-account: {e}"))?;
    let config = build_config(global);

    let mut lanes = Vec::with_capacity(suris.len());
    for (index, suri) in suris.iter().enumerate() {
        let signer = signer_from_suri(suri)?;
        let mut admin = AdminClient::new(config.clone(), signer.clone())
            .context("failed to construct chain client")?;
        admin
            .connect()
            .await
            .with_context(|| format!("failed to connect to chain RPC {}", global.chain_rpc))?;
        lanes.push(Lane {
            index,
            account: AccountId32::from(signer.keypair().public_key().0),
            user: StorageUserClient::new(config.clone(), signer)
                .context("failed to construct provider client")?,
            admin,
            bucket: None,
            live_bytes: 0,
            survivor: None,
        });
    }
    let mut provider = ProviderClient::new(config, signer_from_suri(&suris[0])?)
        .context("failed to construct chain client")?;
    provider
        .connect()
        .await
        .with_context(|| format!("failed to connect to chain RPC {}", global.chain_rpc))?;

    let price_per_byte = preflight(
        &lanes[0].user,
        &provider,
        &provider_account,
        &global.provider_url,
    )
    .await?;
    eprintln!(
        "Each bucket's agreement costs {} ({} bytes x {} blocks at {} per byte-block)",
        u128::from(plan.max_bytes) * u128::from(args.agreement_duration.get()) * price_per_byte,
        plan.max_bytes,
        args.agreement_duration,
        price_per_byte,
    );

    let ctx = GcContext {
        plan,
        provider_url: global.provider_url.clone(),
        provider_account: args.provider_account.clone(),
        agreement_duration: args.agreement_duration.get(),
        price_per_byte,
        run_id: format!(
            "{:x}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_millis())
        ),
        stop: StopSignal::new(),
    };

    let started = Instant::now();
    let mut cycles = Vec::new();
    let mut failure = None;
    for lane in &mut lanes {
        let result = async {
            open_bucket(lane, &ctx).await?;
            run_cycle(lane, &ctx, 0, 0).await
        }
        .await;
        match result {
            Ok(record) => cycles.push(record),
            Err(e) => {
                failure = Some(SoakFailure {
                    message: format!("{e:#}"),
                    epoch: 0,
                    lane: Some(lane.index),
                    cycle: Some(0),
                });
                break;
            }
        }
    }
    Ok(SoakReport {
        service: "gc",
        cycles,
        barriers: Vec::new(),
        failure,
        elapsed: started.elapsed(),
    })
}

/// Fail before any transaction when the provider holds data, or is not
/// registered and accepting primary agreements. Returns its price per byte.
async fn preflight(
    user: &StorageUserClient,
    provider: &ProviderClient,
    provider_account: &AccountId32,
    provider_url: &str,
) -> Result<u128> {
    let stats = user
        .provider_stats()
        .await
        .with_context(|| format!("failed to read {provider_url}/stats"))?;
    if stats.total_buckets != 0 || stats.total_nodes != 0 || stats.total_bytes != 0 {
        bail!(
            "provider {provider_url} is not empty ({} buckets, {} nodes, {} bytes): \
             start it with a fresh storage path",
            stats.total_buckets,
            stats.total_nodes,
            stats.total_bytes
        );
    }
    let info = provider
        .get_provider_info(provider_account)
        .await
        .context("failed to read the provider registration")?
        .context("the provider account is not registered: run `storage-cli provider register`")?;
    ensure!(
        info.accepting_primary,
        "the provider is not accepting primary agreements"
    );
    Ok(info.price_per_byte)
}

/// Open a bucket with a new agreement and reset the lane's expected usage.
async fn open_bucket(lane: &mut Lane, ctx: &GcContext) -> Result<()> {
    let report = prepare::prepare_buckets(
        &lane.admin,
        PrepareParams {
            owner: lane.account.clone(),
            provider: &ctx.provider_account,
            provider_url: &ctx.provider_url,
            count: 1,
            max_bytes: ctx.plan.max_bytes,
            duration: ctx.agreement_duration,
            price_per_byte: ctx.price_per_byte,
        },
    )
    .await?;
    lane.bucket = Some(report.buckets[0]);
    lane.live_bytes = 0;
    lane.survivor = None;
    Ok(())
}

/// The provider answers an upload or its commit over quota with 507.
async fn expect_quota_rejection(lane: &Lane, bucket: BucketId, payload: &[u8]) -> Result<()> {
    let rejected = |e: &ClientError| matches!(e, ClientError::HttpStatus { status: 507, .. });
    match lane
        .user
        .upload(bucket, payload, ChunkingStrategy::default())
        .await
    {
        Err(e) if rejected(&e) => Ok(()),
        Err(e) => bail!("the over-quota upload failed with {e}, not 507"),
        Ok(root) => match lane.user.commit(bucket, vec![root]).await {
            Err(e) if rejected(&e) => Ok(()),
            Err(e) => bail!("the over-quota commit failed with {e}, not 507"),
            Ok(_) => bail!("a chunk larger than the remaining capacity was accepted"),
        },
    }
}

/// One prune cycle on the lane's bucket: fill the quota, checkpoint, check
/// usage and the over-quota rejection, prune every chunk but the newest, then
/// wait for the garbage collector to erase the pruned bytes.
async fn run_cycle(
    lane: &mut Lane,
    ctx: &GcContext,
    epoch: u32,
    cycle: u32,
) -> Result<CycleRecord> {
    let plan = &ctx.plan;
    let bucket = lane.bucket.context("the lane has no bucket")?;
    let provider = ctx.provider_account.clone();
    let chunk_size = plan.chunk_bytes as usize;

    let mut hashes = Vec::new();
    let mut newest_leaf = 0;
    for index in 0..plan.chunks_per_cycle {
        let payload = chunk_payload(&ctx.run_id, lane.index, epoch, cycle, index, chunk_size);
        let root = lane
            .user
            .upload(bucket, &payload, ChunkingStrategy::default())
            .await
            .context("chunk upload failed")?;
        let commit = lane
            .user
            .commit(bucket, vec![root])
            .await
            .context("chunk commit failed")?;
        // `leaf_indices` counts from the commitment's `start_seq`.
        newest_leaf = commit.start_seq
            + commit
                .leaf_indices
                .first()
                .context("the commit returned no leaf index")?;
        hashes.push(root);
        lane.live_bytes += plan.chunk_bytes;
    }

    let signature = lane
        .user
        .get_checkpoint_signature(bucket)
        .await
        .context("failed to fetch the checkpoint signature")?;
    lane.admin
        .submit_checkpoint(
            bucket,
            Commitment {
                mmr_root: signature.mmr_root,
                start_seq: signature.start_seq,
                leaf_count: signature.leaf_count,
            },
            vec![(provider.clone(), signature.provider_signature)],
        )
        .await
        .context("checkpoint failed")?;

    // The first checkpoint of a bucket arms its quota on the provider.
    let usage = poll_until(
        "quota sync",
        QUOTA_SYNC_TIMEOUT,
        POLL_STEP,
        &ctx.stop,
        || async {
            let usage = lane.user.usage(bucket).await?;
            Ok(usage.quota_synced().then_some(usage))
        },
    )
    .await?;
    ensure!(
        usage.max_bytes == plan.max_bytes,
        "quota is {} bytes, the agreement has {}",
        usage.max_bytes,
        plan.max_bytes
    );
    ensure!(
        usage.used_bytes == lane.live_bytes,
        "provider reports {} used bytes, the stored chunks are {}",
        usage.used_bytes,
        lane.live_bytes
    );

    let over_quota = chunk_payload(
        &ctx.run_id,
        lane.index,
        epoch,
        cycle,
        plan.chunks_per_cycle,
        (plan.max_bytes - lane.live_bytes) as usize + 1,
    );
    expect_quota_rejection(lane, bucket, &over_quota).await?;

    let survivor = *hashes.last().context("the cycle uploaded no chunk")?;
    let pruned: Vec<H256> = lane
        .survivor
        .into_iter()
        .chain(hashes[..hashes.len() - 1].iter().copied())
        .collect();
    let commitment = lane
        .user
        .prune(bucket, newest_leaf)
        .await
        .context("prune failed")?;
    lane.user
        .confirm_deletion(bucket, &commitment)
        .await
        .context("deletion confirmation failed")?;
    lane.admin
        .submit_checkpoint(
            bucket,
            commitment.commitment(),
            vec![(provider, commitment.provider_signature)],
        )
        .await
        .context("prune checkpoint failed")?;

    let erasure_start = Instant::now();
    let after = poll_until(
        "erasure and quota reclaim",
        plan.erase_timeout,
        POLL_STEP,
        &ctx.stop,
        || async {
            let usage = lane.user.usage(bucket).await?;
            Ok((usage.used_bytes == plan.chunk_bytes).then_some(usage))
        },
    )
    .await?;
    let erase_latency = erasure_start.elapsed();
    lane.live_bytes = plan.chunk_bytes;
    lane.survivor = Some(survivor);

    for hash in &pruned {
        match lane.user.read_node(bucket, hash).await {
            Err(ClientError::HttpStatus { status: 404, .. }) => {}
            Err(e) => bail!("reading the erased chunk {hash:#x} failed with {e}, not 404"),
            Ok(_) => bail!("the erased chunk {hash:#x} is still downloadable"),
        }
    }
    lane.user
        .read_node(bucket, &survivor)
        .await
        .context("the surviving chunk is not downloadable")?;

    eprintln!(
        "  lane {} epoch {epoch} cycle {cycle} bucket {bucket}: erased in {:.1}s, used {}/{}",
        lane.index,
        erase_latency.as_secs_f64(),
        after.used_bytes,
        plan.max_bytes,
    );
    Ok(CycleRecord {
        lane: lane.index,
        epoch,
        cycle,
        bucket_id: bucket,
        uploaded_bytes: u64::from(plan.chunks_per_cycle) * plan.chunk_bytes,
        erase_latency_secs: erase_latency.as_secs_f64(),
        used_bytes_after: after.used_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{Cli, Command};
    use crate::commands::soak::Soak;
    use clap::Parser;
    use std::io::Write;

    fn parse(args: &[&str]) -> Result<GcArgs, clap::Error> {
        let base = ["storage-cli", "soak", "gc", "--provider-account", "//Alice"];
        let cli = Cli::try_parse_from(base.iter().chain(args))?;
        match cli.command {
            Command::Soak(Soak::Gc(args)) => Ok(args),
            _ => unreachable!(),
        }
    }

    fn rejected_option(args: &[&str]) -> String {
        parse(args).unwrap_err().to_string()
    }

    #[test]
    fn help_lists_the_options_without_a_chain() {
        let err = Cli::try_parse_from(["storage-cli", "soak", "gc", "--help"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayHelp);
        let help = err.to_string();
        for option in [
            "--client-suri",
            "--cycles-per-bucket",
            "--chunk-bytes",
            "--gc-scan-interval",
            "--provider-log",
        ] {
            assert!(help.contains(option), "help lacks {option}");
        }
    }

    #[test]
    fn a_lane_account_is_required() {
        assert!(parse(&[]).is_err());
    }

    #[test]
    fn invalid_values_name_the_option() {
        assert!(
            rejected_option(&["--client-suri", "//Bob", "--chunks-per-cycle", "1"])
                .contains("--chunks-per-cycle")
        );
        assert!(
            rejected_option(&["--client-suri", "//Bob", "--chunk-bytes", "10"])
                .contains("--chunk-bytes")
        );
        assert!(
            rejected_option(&["--client-suri", "//Bob", "--chunk-bytes", "262144"])
                .contains("--chunk-bytes")
        );
        assert!(
            rejected_option(&["--client-suri", "//Bob", "--duration", "0m"]).contains("--duration")
        );
        assert!(
            rejected_option(&["--client-suri", "//Bob", "--cycles-per-bucket", "0"])
                .contains("--cycles-per-bucket")
        );
        let both = rejected_option(&[
            "--client-suri",
            "//Bob",
            "--duration",
            "5m",
            "--epochs",
            "2",
        ]);
        assert!(
            both.contains("--duration") && both.contains("--epochs"),
            "{both}"
        );
    }

    #[test]
    fn repeated_suris_and_keyfiles_make_one_lane_each() {
        let mut keyfile = tempfile::NamedTempFile::new().unwrap();
        writeln!(keyfile, " //Eve ").unwrap();
        let path = keyfile.path().to_str().unwrap();
        let args = parse(&[
            "--client-suri",
            "//Bob",
            "--client-suri",
            "//Dave",
            "--client-keyfile",
            path,
        ])
        .unwrap();
        assert_eq!(args.lanes.suris().unwrap(), ["//Bob", "//Dave", "//Eve"]);
    }

    #[test]
    fn two_lanes_with_the_same_account_are_rejected() {
        let args = parse(&["--client-suri", "//Bob", "--client-suri", "//Bob"]).unwrap();
        let err = args.lanes.suris().unwrap_err().to_string();
        assert!(err.contains("same account"), "{err}");
    }

    #[test]
    fn plan_defaults_to_a_30_minute_deadline() {
        let plan = parse(&["--client-suri", "//Bob"]).unwrap().plan();
        assert_eq!(plan.stop, StopAt::Deadline(Duration::from_secs(1800)));
        assert_eq!(plan.chunk_bytes, 100);
        assert_eq!(plan.cycles_per_bucket, 5);
    }

    #[test]
    fn plan_sizes_the_quota_and_timeouts_from_the_options() {
        let plan = parse(&[
            "--client-suri",
            "//Bob",
            "--epochs",
            "4",
            "--chunks-per-cycle",
            "5",
            "--chunk-bytes",
            "200",
            "--gc-scan-interval",
            "45",
            "--erase-timeout",
            "90",
        ])
        .unwrap()
        .plan();
        assert_eq!(plan.stop, StopAt::Epochs(4));
        assert_eq!(plan.max_bytes, 6 * 200);
        assert_eq!(plan.drain_timeout, Duration::from_secs(2 * 45 + 60));
        assert_eq!(plan.erase_timeout, Duration::from_secs(90));
    }

    #[test]
    fn chunk_payload_has_the_requested_size() {
        assert_eq!(chunk_payload("r1", 0, 0, 0, 0, 100).len(), 100);
        assert_eq!(chunk_payload("r1", 3, 12, 7, 2, 64).len(), 64);
    }

    #[test]
    fn chunk_payload_differs_in_every_coordinate() {
        let base = chunk_payload("r1", 1, 2, 3, 4, 100);
        for other in [
            chunk_payload("r2", 1, 2, 3, 4, 100),
            chunk_payload("r1", 0, 2, 3, 4, 100),
            chunk_payload("r1", 1, 0, 3, 4, 100),
            chunk_payload("r1", 1, 2, 0, 4, 100),
            chunk_payload("r1", 1, 2, 3, 0, 100),
        ] {
            assert_ne!(base, other);
        }
        assert_ne!(
            chunk_payload("r1", 1, 12, 3, 4, 100),
            chunk_payload("r1", 11, 2, 3, 4, 100)
        );
    }
}
