// SPDX-License-Identifier: Apache-2.0

//! Upload action: perform a single off-chain HTTP upload and measure it.

use std::time::Instant;

use storage_client::{ChunkingStrategy, StorageUserClient};

use crate::common::BucketId;
use crate::metrics::{OpLabels, OpOutcome, Operation};

/// Off-chain HTTP upload to a provider.
pub struct Upload;

impl Operation for Upload {
    fn labels(&self) -> OpLabels {
        OpLabels {
            verb: "upload",
            noun_plural: "uploads",
            past_tense: "uploaded",
        }
    }
}

/// Perform one upload of `payload` to `bucket`, returning the measured outcome.
pub async fn upload_once(
    client: &StorageUserClient,
    bucket: BucketId,
    payload: &[u8],
) -> OpOutcome {
    let started = Instant::now();
    match client
        .upload(bucket, payload, ChunkingStrategy::default())
        .await
    {
        Ok(_root) => OpOutcome::success(payload.len(), started.elapsed()),
        Err(e) => OpOutcome::failure(payload.len(), started.elapsed(), e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upload_labels_match() {
        let l = Upload.labels();
        assert_eq!(l.verb, "upload");
        assert_eq!(l.noun_plural, "uploads");
        assert_eq!(l.past_tense, "uploaded");
    }
}
