// SPDX-License-Identifier: Apache-2.0

//! Authentication wrapper. Actual logics stand at provider_auth

use crate::error::Error;
use crate::ProviderState;
use provider_auth::LocalBucketAgreement;
use provider_storage::BucketInfo;
use storage_primitives::{BucketId, Role};

/// Extract the Authorization header value from a header map.
fn auth_header(headers: &axum::http::HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
}

pub(crate) async fn require_role(
    state: &ProviderState,
    headers: &axum::http::HeaderMap,
    method: &str,
    bucket_id: u64,
    required: Role,
) -> Result<(), Error> {
    state
        .auth
        .require_role(auth_header(headers), method, bucket_id, required)
        .await
        .map_err(Into::into)
}

pub(crate) async fn require_agreement(
    state: &ProviderState,
    bucket_id: BucketId,
    required: Role,
) -> Result<LocalBucketAgreement, Error> {
    let anchor_block = state
        .chain_state
        .current_anchor_block
        .load(std::sync::atomic::Ordering::Relaxed);
    state
        .auth
        .require_agreement(bucket_id, required, anchor_block)
        .await
        .map_err(Into::into)
}

/// The local bucket, once this node's agreement on it allows the request.
pub(crate) async fn require_bucket(
    state: &ProviderState,
    bucket_id: BucketId,
    required: Role,
) -> Result<BucketInfo, Error> {
    // coupling agreement validity with local bucket storage
    require_agreement(state, bucket_id, required).await?;
    Ok(state
        .blocking_storage(move |storage| storage.get_bucket(bucket_id))
        .await??
        .ok_or(provider_storage::Error::BucketNotFound(bucket_id))?)
}
