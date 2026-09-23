// SPDX-License-Identifier: Apache-2.0

//! Setup phase of `stress-test upload`: open the buckets and agreements a run
//! needs before the clock starts.
//!
//! Nothing here produces an [`crate::metrics::OpOutcome`], so the reported
//! numbers cover uploads only. A failure aborts the run instead of counting as
//! a failed upload.

use std::fmt;
use std::num::{NonZeroU64, NonZeroUsize};
use std::time::Instant;

use anyhow::{bail, Context, Result};
use clap::ValueEnum;
use serde::{Serialize, Serializer};
use sp_runtime::AccountId32;
use storage_client::{AdminClient, NegotiateRequest, ProviderClient, Visibility};

use crate::common::BucketId;

/// Where `stress-test upload` takes its target buckets from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum BucketSource {
    /// Only buckets the account already has an agreement with the provider for.
    Discover,
    /// Only buckets prepared by this run (`--prepare-buckets` required).
    Create,
    /// Discovered buckets first; prepare more only if fewer than
    /// `--prepare-buckets` exist.
    Auto,
}

impl BucketSource {
    fn name(self) -> &'static str {
        match self {
            Self::Discover => "discover",
            Self::Create => "create",
            Self::Auto => "auto",
        }
    }

    /// Resolve the effective source. Without an explicit `--bucket-source`,
    /// `--prepare-buckets N` means `create`, otherwise `discover`.
    pub fn resolve(explicit: Option<Self>, prepare_buckets: usize) -> Result<Self> {
        match (explicit, prepare_buckets) {
            (None, 0) | (Some(Self::Discover), 0) => Ok(Self::Discover),
            (None, _) => Ok(Self::Create),
            (Some(Self::Discover), _) => {
                bail!("--prepare-buckets needs --bucket-source create or auto")
            }
            (Some(source), 0) => bail!(
                "--bucket-source {} needs --prepare-buckets >= 1",
                source.name()
            ),
            (Some(source), _) => Ok(source),
        }
    }
}

/// The target set of a run: buckets already usable, plus how many to prepare.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetPlan {
    pub existing: Vec<BucketId>,
    pub to_prepare: usize,
}

/// Decide which discovered buckets to keep and how many to prepare.
///
/// `--max-buckets-to-write` is applied here, before anything is created, so a
/// bucket this run pays to open is never truncated away afterwards.
pub fn plan_targets(
    source: BucketSource,
    requested: usize,
    mut discovered: Vec<BucketId>,
    cap: Option<NonZeroUsize>,
) -> Result<TargetPlan> {
    let cap = cap.map(NonZeroUsize::get);
    match source {
        BucketSource::Discover => {
            if let Some(cap) = cap {
                discovered.truncate(cap);
            }
            Ok(TargetPlan {
                existing: discovered,
                to_prepare: 0,
            })
        }
        BucketSource::Create => {
            if let Some(cap) = cap {
                if cap < requested {
                    bail!(
                        "--max-buckets-to-write {cap} is below --prepare-buckets {requested}: \
                         every prepared bucket must receive uploads"
                    );
                }
            }
            Ok(TargetPlan {
                existing: Vec::new(),
                to_prepare: requested,
            })
        }
        BucketSource::Auto => {
            if let Some(cap) = cap {
                discovered.truncate(cap);
            }
            let target = cap.map_or(requested, |cap| requested.min(cap));
            let to_prepare = target.saturating_sub(discovered.len());
            Ok(TargetPlan {
                existing: discovered,
                to_prepare,
            })
        }
    }
}

/// Bytes the configured load writes to the busiest bucket. Uploads are
/// assigned round-robin, so a bucket receives at most
/// `ceil(total_uploads / bucket_count)` payloads.
pub fn bytes_per_bucket(
    total_uploads: usize,
    payload_size: usize,
    bucket_count: usize,
) -> Result<u64> {
    if bucket_count == 0 {
        bail!("no target buckets");
    }
    let uploads = u64::try_from(total_uploads.div_ceil(bucket_count))?;
    let payload = u64::try_from(payload_size)?;
    uploads
        .checked_mul(payload)
        .context("configured load overflows u64 bytes")
}

/// Default agreement quota: the per-bucket load plus 10% headroom for the
/// Merkle tree nodes the provider stores next to the chunks.
pub fn default_max_bytes(needed: u64) -> Result<u64> {
    needed
        .checked_add(needed / 10)
        .context("agreement quota overflows u64")
}

/// `--prepare-max-bytes` without `--prepare-buckets` would be ignored; reject
/// it, like `--bucket-source create` without `--prepare-buckets`.
pub fn check_prepare_flags(
    prepare_buckets: usize,
    prepare_max_bytes: Option<NonZeroU64>,
) -> Result<()> {
    if prepare_buckets == 0 && prepare_max_bytes.is_some() {
        bail!("--prepare-max-bytes needs --prepare-buckets >= 1");
    }
    Ok(())
}

/// The quota to negotiate: the explicit `--prepare-max-bytes` if it covers the
/// per-bucket load, else the default. Checked up front because the provider
/// would accept the agreement and then reject uploads past the quota.
pub fn resolve_max_bytes(given: Option<NonZeroU64>, needed: u64) -> Result<u64> {
    match given {
        Some(given) if given.get() < needed => bail!(
            "--prepare-max-bytes {given} is below the {needed} bytes this run writes to a \
             single bucket"
        ),
        Some(given) => Ok(given.get()),
        None => default_max_bytes(needed),
    }
}

/// Terms of the agreements to open.
pub struct PrepareParams<'a> {
    /// Account that pays for and owns the prepared buckets.
    pub owner: AccountId32,
    /// Provider account as given on the command line (SS58 or `0x`-hex).
    pub provider: &'a str,
    pub provider_url: &'a str,
    pub count: usize,
    pub max_bytes: u64,
    /// Agreement duration in anchor blocks.
    pub duration: u32,
    pub price_per_byte: u128,
}

/// What preparation created. Serialized as its own `preparation` object in the
/// JSON output; `results` covers uploads only.
#[derive(Debug, Clone, Serialize)]
pub struct PreparationReport {
    pub buckets: Vec<BucketId>,
    pub max_bytes: u64,
    pub duration: u32,
    /// A decimal string, like the negotiation wire format, so JavaScript
    /// readers keep full precision.
    #[serde(serialize_with = "u128_as_string")]
    pub price_per_byte: u128,
    pub elapsed_secs: f64,
}

fn u128_as_string<S: Serializer>(value: &u128, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&value.to_string())
}

/// Negotiate terms with the provider node and redeem them on-chain, once per
/// bucket. Same SDK path as the `complete_workflow` example. `admin` must be
/// connected.
pub async fn prepare_buckets(
    admin: &AdminClient,
    params: PrepareParams<'_>,
) -> Result<PreparationReport> {
    let started = Instant::now();

    eprintln!(
        "Preparing {} bucket(s): {} bytes x {} blocks at {} per byte-block via {}",
        params.count, params.max_bytes, params.duration, params.price_per_byte, params.provider_url,
    );

    let mut buckets = Vec::with_capacity(params.count);
    for i in 1..=params.count {
        let signed = ProviderClient::negotiate_terms(
            params.provider_url,
            &NegotiateRequest {
                owner: params.owner.clone(),
                max_bytes: params.max_bytes,
                duration: params.duration,
                price_per_byte: params.price_per_byte,
                bucket_id: None,
                replica_params: None,
            },
        )
        .await
        .with_context(|| {
            format!(
                "provider {} rejected terms for bucket {i}/{}",
                params.provider_url, params.count
            )
        })?;

        let bucket_id = admin
            .establish_storage_agreement(params.provider.to_string(), signed, Visibility::Private)
            .await
            .with_context(|| {
                format!(
                    "failed to establish agreement for bucket {i}/{}",
                    params.count
                )
            })?;
        eprintln!("  prepared bucket #{bucket_id} ({i}/{})", params.count);
        buckets.push(bucket_id);
    }

    Ok(PreparationReport {
        buckets,
        max_bytes: params.max_bytes,
        duration: params.duration,
        price_per_byte: params.price_per_byte,
        elapsed_secs: started.elapsed().as_secs_f64(),
    })
}

impl fmt::Display for PreparationReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "preparation: {} bucket(s) created in {:.3}s",
            self.buckets.len(),
            self.elapsed_secs
        )?;
        let ids: Vec<String> = self.buckets.iter().map(|b| format!("#{b}")).collect();
        writeln!(f, "  buckets:    {}", ids.join(", "))?;
        writeln!(
            f,
            "  agreement:  {} bytes x {} blocks at {} per byte-block",
            self.max_bytes, self.duration, self.price_per_byte
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cap(n: usize) -> Option<NonZeroUsize> {
        NonZeroUsize::new(n)
    }

    #[test]
    fn source_defaults_follow_prepare_buckets() {
        assert_eq!(
            BucketSource::resolve(None, 0).unwrap(),
            BucketSource::Discover
        );
        assert_eq!(
            BucketSource::resolve(None, 3).unwrap(),
            BucketSource::Create
        );
    }

    #[test]
    fn explicit_source_is_validated_against_prepare_buckets() {
        assert!(BucketSource::resolve(Some(BucketSource::Discover), 2).is_err());
        assert!(BucketSource::resolve(Some(BucketSource::Create), 0).is_err());
        assert!(BucketSource::resolve(Some(BucketSource::Auto), 0).is_err());
        assert_eq!(
            BucketSource::resolve(Some(BucketSource::Auto), 2).unwrap(),
            BucketSource::Auto
        );
    }

    #[test]
    fn discover_keeps_found_buckets_up_to_cap() {
        let plan = plan_targets(BucketSource::Discover, 0, vec![1, 2, 3], cap(2)).unwrap();
        assert_eq!(
            plan,
            TargetPlan {
                existing: vec![1, 2],
                to_prepare: 0
            }
        );
    }

    #[test]
    fn create_prepares_exactly_requested() {
        let plan = plan_targets(BucketSource::Create, 3, vec![9], None).unwrap();
        assert_eq!(
            plan,
            TargetPlan {
                existing: vec![],
                to_prepare: 3
            }
        );
    }

    #[test]
    fn create_rejects_cap_below_requested() {
        assert!(plan_targets(BucketSource::Create, 3, vec![], cap(2)).is_err());
    }

    #[test]
    fn auto_prepares_only_the_shortfall() {
        let plan = plan_targets(BucketSource::Auto, 3, vec![1], None).unwrap();
        assert_eq!(plan.existing, vec![1]);
        assert_eq!(plan.to_prepare, 2);

        let plan = plan_targets(BucketSource::Auto, 3, vec![1, 2, 3, 4, 5], None).unwrap();
        assert_eq!(plan.existing.len(), 5);
        assert_eq!(plan.to_prepare, 0);
    }

    #[test]
    fn auto_never_exceeds_cap() {
        let plan = plan_targets(BucketSource::Auto, 5, vec![1, 2, 3], cap(4)).unwrap();
        assert_eq!(plan.existing, vec![1, 2, 3]);
        assert_eq!(plan.to_prepare, 1);
    }

    #[test]
    fn bytes_per_bucket_uses_the_busiest_bucket() {
        // 10 uploads over 3 buckets: round-robin gives one bucket 4 payloads.
        assert_eq!(bytes_per_bucket(10, 1_048_576, 3).unwrap(), 4 * 1_048_576);
        assert_eq!(bytes_per_bucket(1, 512, 1).unwrap(), 512);
        assert!(bytes_per_bucket(1, 512, 0).is_err());
    }

    #[test]
    fn default_quota_adds_ten_percent() {
        assert_eq!(default_max_bytes(1_000).unwrap(), 1_100);
        assert!(default_max_bytes(u64::MAX).is_err());
    }

    #[test]
    fn explicit_quota_must_cover_the_load() {
        let given = |n: u64| NonZeroU64::new(n);
        assert!(resolve_max_bytes(given(999), 1_000).is_err());
        assert_eq!(resolve_max_bytes(given(1_000), 1_000).unwrap(), 1_000);
        assert_eq!(resolve_max_bytes(given(5_000), 1_000).unwrap(), 5_000);
        assert_eq!(resolve_max_bytes(None, 1_000).unwrap(), 1_100);
    }

    #[test]
    fn quota_flag_needs_prepare_buckets() {
        assert!(check_prepare_flags(0, NonZeroU64::new(1)).is_err());
        assert!(check_prepare_flags(0, None).is_ok());
        assert!(check_prepare_flags(2, NonZeroU64::new(1)).is_ok());
    }
}
