// SPDX-License-Identifier: Apache-2.0

//! The leaf phase of a replica sync: get the MMR leaves of the chain's target
//! commitment and check them against its root before any chunk moves.

use crate::source::{SourceClient, SourceSet};
use crate::Error;
use provider_storage::StorageBackend;
use storage_primitives::{
    mmr::{self, MmrLeaf},
    BucketId, Commitment,
};

/// The leaves of `target`, checked against `target.mmr_root`.
///
/// Local leaves that fall in the target's range are kept, and only the missing
/// range is fetched, in pages, from the first usable source. A failed page is
/// retried from its first sequence number on the next source.
///
/// If the root does not match, the replica cannot tell which leaves are wrong,
/// the kept ones or the fetched ones. It drops the kept leaves and fetches the
/// whole range from one source at a time. A source whose leaves do not match
/// the root is dropped, and the next one is tried.
///
/// Fails with `NoSourceLeft` when every source has failed.
pub async fn fetch_leaves(
    storage: &dyn StorageBackend,
    client: &SourceClient,
    sources: &mut SourceSet,
    bucket_id: BucketId,
    target: &Commitment,
) -> Result<Vec<MmrLeaf>, Error> {
    let end = target.start_seq.saturating_add(target.leaf_count);

    let mut leaves = kept_leaves(storage, bucket_id, target, end)?;
    let mut next = target.start_seq + leaves.len() as u64;
    while next < end {
        let source = current_source(sources)?;
        match client.mmr_leaves(&source, bucket_id, next, end).await {
            Ok(page) => {
                leaves.extend(page.leaves);
                next = page.next_seq;
            }
            Err(e) => {
                tracing::warn!(
                    bucket_id,
                    source,
                    "leaf page failed, trying the next source: {e}"
                );
                sources.drop_source(&source);
            }
        }
    }
    if matches_root(&leaves, target)? {
        return Ok(leaves);
    }

    tracing::warn!(
        bucket_id,
        "leaves do not match the target root, refetching the range"
    );
    loop {
        let source = current_source(sources)?;
        match fetch_range(client, &source, bucket_id, target.start_seq, end).await {
            Ok(leaves) if matches_root(&leaves, target)? => return Ok(leaves),
            Ok(_) => tracing::warn!(
                bucket_id,
                source,
                "source served leaves that do not match the target root"
            ),
            Err(e) => tracing::warn!(bucket_id, source, "leaf range failed: {e}"),
        }
        sources.drop_source(&source);
    }
}

/// The local leaves that start the target's range: from `target.start_seq` up
/// to the end of the local leaves or of the target, whichever comes first.
/// Local leaves before `target.start_seq` were deleted from the target and are
/// dropped. A local range that does not reach `target.start_seq` leaves a gap,
/// so nothing is kept.
fn kept_leaves(
    storage: &dyn StorageBackend,
    bucket_id: BucketId,
    target: &Commitment,
    end: u64,
) -> Result<Vec<MmrLeaf>, Error> {
    let Some(local) = storage.get_bucket(bucket_id) else {
        return Ok(Vec::new());
    };
    let local_end = local.start_seq.saturating_add(local.leaf_count);
    if local.start_seq > target.start_seq || local_end <= target.start_seq {
        return Ok(Vec::new());
    }
    let page = storage.get_leaves(
        bucket_id,
        target.start_seq,
        local_end.min(end),
        usize::MAX,
        usize::MAX,
    )?;
    Ok(page.leaves)
}

/// The leaves in `[from, to)`, all from `source`.
async fn fetch_range(
    client: &SourceClient,
    source: &str,
    bucket_id: BucketId,
    from: u64,
    to: u64,
) -> Result<Vec<MmrLeaf>, Error> {
    let mut leaves = Vec::new();
    let mut next = from;
    while next < to {
        let page = client.mmr_leaves(source, bucket_id, next, to).await?;
        leaves.extend(page.leaves);
        next = page.next_seq;
    }
    Ok(leaves)
}

fn matches_root(leaves: &[MmrLeaf], target: &Commitment) -> Result<bool, Error> {
    let root = mmr::root(leaves).map_err(|e| Error::Backend(e.into()))?;
    Ok(root == target.mmr_root)
}

fn current_source(sources: &SourceSet) -> Result<String, Error> {
    sources
        .current()
        .map(str::to_owned)
        .ok_or(Error::NoSourceLeft { what: "mmr leaves" })
}
