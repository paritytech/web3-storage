// SPDX-License-Identifier: GPL-3.0-only

//! Error types for the provider node.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use provider_auth::{AuthError, MembershipError};
use serde::Serialize;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Node not found: {0}")]
    NodeNotFound(String),

    #[error("Children missing: {0:?}")]
    ChildrenMissing(Vec<String>),

    #[error("Quota exceeded: used {used}, max {max}")]
    QuotaExceeded { used: u64, max: u64 },

    #[error("Bucket not found: {0}")]
    BucketNotFound(u64),

    #[error("Root not found: {0}")]
    RootNotFound(String),

    #[error("Invalid hash: expected {expected}, got {actual}")]
    InvalidHash { expected: String, actual: String },

    #[error("Invalid signature")]
    InvalidSignature,

    #[error("Not authorized: {0}")]
    NotAuthorized(String),

    #[error("Storage error: {0}")]
    Storage(String),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Internal error: {0}")]
    Internal(String),

    #[error("Object not found: bucket {bucket_id}, key {key}")]
    ObjectNotFound { bucket_id: u64, key: String },

    #[error("Invalid object key: {0}")]
    InvalidObjectKey(String),

    #[error("File not found: bucket {bucket_id}, path {path}")]
    FileNotFound { bucket_id: u64, path: String },

    #[error("Not a file: bucket {bucket_id}, path {path}")]
    NotAFile { bucket_id: u64, path: String },

    #[error("Invalid path: {0}")]
    InvalidPath(String),

    #[error(transparent)]
    Auth(#[from] AuthError),

    #[error("Signing unavailable: provider has no keypair configured")]
    SigningUnavailable,

    #[error("Nonce counter unavailable; provider has not bootstrapped replay state")]
    NonceCounterUnavailable,

    #[error("Provider is not accepting new primary agreements")]
    NotAcceptingPrimary,

    #[error("Provider is not accepting replica agreements")]
    NotAcceptingReplicas,

    #[error("Proposed price_per_byte {proposed} is below the provider's listed price {listed}")]
    PriceBelowListed { proposed: u128, listed: u128 },

    #[error("Duration {duration} is outside the provider's bounds [{min}, {max}]")]
    DurationOutOfBounds { duration: u32, min: u32, max: u32 },

    #[error(
        "Requested {requested} bytes exceeds remaining capacity \
         ({committed} of {max_capacity} bytes committed)"
    )]
    CapacityExceeded {
        requested: u64,
        committed: u64,
        max_capacity: u64,
    },

    #[error("Provider on-chain info unavailable; cannot validate terms")]
    ProviderInfoUnavailable,

    #[error("Provider is deregistering; not accepting new agreements")]
    ProviderDeregistering,

    #[error(
        "Chain state not ready: current_anchor_block and request_timeout must both be non-zero"
    )]
    ChainStateNotReady,

    #[error("Storage agreement requested 0 byte")]
    InvalidMaxBytesRequest,

    #[error("Too many requests")]
    RateLimited,
}

/// Chain-connection failures are internal: the node cannot reach the chain,
/// which is never the caller's fault.
impl From<provider_chain::Error> for Error {
    fn from(e: provider_chain::Error) -> Self {
        Error::Internal(e.to_string())
    }
}

/// Map storage-engine errors onto the node's error space one-to-one so the
/// HTTP status/JSON mapping below stays the single source of truth.
impl From<provider_storage::Error> for Error {
    fn from(e: provider_storage::Error) -> Self {
        use provider_storage::Error as StorageError;
        match e {
            StorageError::NodeNotFound(hash) => Error::NodeNotFound(hash),
            StorageError::ChildrenMissing(children) => Error::ChildrenMissing(children),
            StorageError::QuotaExceeded { used, max } => Error::QuotaExceeded { used, max },
            StorageError::BucketNotFound(id) => Error::BucketNotFound(id),
            StorageError::RootNotFound(root) => Error::RootNotFound(root),
            StorageError::InvalidHash { expected, actual } => {
                Error::InvalidHash { expected, actual }
            }
            StorageError::Storage(msg) => Error::Storage(msg),
            StorageError::Serialization(msg) => Error::Serialization(msg),
        }
    }
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<serde_json::Value>,
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let (status, error_response) = match &self {
            Error::NodeNotFound(hash) => (
                StatusCode::NOT_FOUND,
                ErrorResponse {
                    error: "not_found".to_string(),
                    details: Some(serde_json::json!({ "hash": hash })),
                },
            ),
            Error::ChildrenMissing(children) => (
                StatusCode::BAD_REQUEST,
                ErrorResponse {
                    error: "children_missing".to_string(),
                    details: Some(serde_json::json!({ "missing": children })),
                },
            ),
            Error::QuotaExceeded { used, max } => (
                StatusCode::INSUFFICIENT_STORAGE,
                ErrorResponse {
                    error: "quota_exceeded".to_string(),
                    details: Some(serde_json::json!({ "used": used, "max": max })),
                },
            ),
            Error::BucketNotFound(id) => (
                StatusCode::NOT_FOUND,
                ErrorResponse {
                    error: "bucket_not_found".to_string(),
                    details: Some(serde_json::json!({ "bucket_id": id })),
                },
            ),
            Error::RootNotFound(root) => (
                StatusCode::NOT_FOUND,
                ErrorResponse {
                    error: "root_not_found".to_string(),
                    details: Some(serde_json::json!({ "data_root": root })),
                },
            ),
            Error::InvalidHash { expected, actual } => (
                StatusCode::BAD_REQUEST,
                ErrorResponse {
                    error: "invalid_hash".to_string(),
                    details: Some(serde_json::json!({
                        "expected": expected,
                        "actual": actual
                    })),
                },
            ),
            Error::InvalidSignature => (
                StatusCode::BAD_REQUEST,
                ErrorResponse {
                    error: "invalid_signature".to_string(),
                    details: None,
                },
            ),
            Error::NotAuthorized(reason) => (
                StatusCode::FORBIDDEN,
                ErrorResponse {
                    error: "not_authorized".to_string(),
                    details: Some(serde_json::json!({ "reason": reason })),
                },
            ),
            Error::Storage(msg) | Error::Internal(msg) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorResponse {
                    error: "internal_error".to_string(),
                    details: Some(serde_json::json!({ "message": msg })),
                },
            ),
            Error::Serialization(msg) => (
                StatusCode::BAD_REQUEST,
                ErrorResponse {
                    error: "serialization_error".to_string(),
                    details: Some(serde_json::json!({ "message": msg })),
                },
            ),
            Error::ObjectNotFound { bucket_id, key } => (
                StatusCode::NOT_FOUND,
                ErrorResponse {
                    error: "object_not_found".to_string(),
                    details: Some(serde_json::json!({ "bucket_id": bucket_id, "key": key })),
                },
            ),
            Error::InvalidObjectKey(key) => (
                StatusCode::BAD_REQUEST,
                ErrorResponse {
                    error: "invalid_object_key".to_string(),
                    details: Some(serde_json::json!({ "key": key })),
                },
            ),
            Error::FileNotFound { bucket_id, path } => (
                StatusCode::NOT_FOUND,
                ErrorResponse {
                    error: "file_not_found".to_string(),
                    details: Some(serde_json::json!({ "bucket_id": bucket_id, "path": path })),
                },
            ),
            Error::NotAFile { bucket_id, path } => (
                StatusCode::BAD_REQUEST,
                ErrorResponse {
                    error: "not_a_file".to_string(),
                    details: Some(serde_json::json!({ "bucket_id": bucket_id, "path": path })),
                },
            ),
            Error::InvalidPath(msg) => (
                StatusCode::BAD_REQUEST,
                ErrorResponse {
                    error: "invalid_path".to_string(),
                    details: Some(serde_json::json!({ "message": msg })),
                },
            ),
            Error::Auth(AuthError::AuthRequired | AuthError::TimestampExpired) => (
                StatusCode::UNAUTHORIZED,
                ErrorResponse {
                    error: "auth_required".to_string(),
                    details: Some(serde_json::json!({ "message": self.to_string() })),
                },
            ),
            Error::Auth(AuthError::InsufficientRole) => (
                StatusCode::FORBIDDEN,
                ErrorResponse {
                    error: "insufficient_role".to_string(),
                    details: None,
                },
            ),
            // Transient and worth retrying, unlike a decode failure.
            Error::Auth(err @ AuthError::MembershipLookup(MembershipError::Unavailable(_))) => (
                StatusCode::SERVICE_UNAVAILABLE,
                ErrorResponse {
                    error: "membership_unavailable".to_string(),
                    details: Some(serde_json::json!({ "message": err.to_string() })),
                },
            ),
            Error::Auth(err @ AuthError::MembershipLookup(_)) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorResponse {
                    error: "internal_error".to_string(),
                    details: Some(serde_json::json!({ "message": err.to_string() })),
                },
            ),
            Error::SigningUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                ErrorResponse {
                    error: "signing_unavailable".to_string(),
                    details: Some(serde_json::json!({
                        "message": "provider node signer is not available."
                    })),
                },
            ),
            Error::NonceCounterUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                ErrorResponse {
                    error: "nonce_counter_unavailable".to_string(),
                    details: Some(serde_json::json!({
                        "message": "provider node has not bootstrapped its nonce counter from \
                                    on-chain replay state; ensure the provider is registered and \
                                    the chain is reachable, then retry"
                    })),
                },
            ),
            Error::NotAcceptingPrimary => (
                StatusCode::UNPROCESSABLE_ENTITY,
                ErrorResponse {
                    error: "not_accepting_primary".to_string(),
                    details: None,
                },
            ),
            Error::NotAcceptingReplicas => (
                StatusCode::UNPROCESSABLE_ENTITY,
                ErrorResponse {
                    error: "not_accepting_replicas".to_string(),
                    details: None,
                },
            ),
            Error::PriceBelowListed { proposed, listed } => (
                StatusCode::UNPROCESSABLE_ENTITY,
                ErrorResponse {
                    error: "price_below_listed".to_string(),
                    // u128 doesn't fit serde_json numbers; send as strings.
                    details: Some(serde_json::json!({
                        "proposed": proposed.to_string(),
                        "listed": listed.to_string(),
                    })),
                },
            ),
            Error::DurationOutOfBounds { duration, min, max } => (
                StatusCode::UNPROCESSABLE_ENTITY,
                ErrorResponse {
                    error: "duration_out_of_bounds".to_string(),
                    details: Some(serde_json::json!({
                        "duration": duration,
                        "min": min,
                        "max": max,
                    })),
                },
            ),
            Error::CapacityExceeded {
                requested,
                committed,
                max_capacity,
            } => (
                StatusCode::UNPROCESSABLE_ENTITY,
                ErrorResponse {
                    error: "capacity_exceeded".to_string(),
                    details: Some(serde_json::json!({
                        "requested": requested,
                        "committed": committed,
                        "max_capacity": max_capacity,
                    })),
                },
            ),
            Error::InvalidMaxBytesRequest => (
                StatusCode::UNPROCESSABLE_ENTITY,
                ErrorResponse {
                    error: "invalid_max_bytes_request".to_string(),
                    details: None,
                },
            ),
            Error::ProviderInfoUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                ErrorResponse {
                    error: "provider_info_unavailable".to_string(),
                    details: Some(serde_json::json!({
                        "message": "provider's on-chain registration info is not loaded; \
                                    cannot validate agreement terms"
                    })),
                },
            ),
            Error::ChainStateNotReady => (
                StatusCode::SERVICE_UNAVAILABLE,
                ErrorResponse {
                    error: "chain_state_not_ready".to_string(),
                    details: Some(serde_json::json!({
                        "message": "current_anchor_block or request_timeout is 0; \
                                    the node has not yet synced with the chain"
                    })),
                },
            ),
            Error::ProviderDeregistering => (
                StatusCode::SERVICE_UNAVAILABLE,
                ErrorResponse {
                    error: "provider_deregistering".to_string(),
                    details: Some(serde_json::json!({
                        "message": "provider has announced deregistration and is no \
                                    longer accepting new storage agreements"
                    })),
                },
            ),
            Error::RateLimited => (
                StatusCode::TOO_MANY_REQUESTS,
                ErrorResponse {
                    error: "rate_limited".to_string(),
                    details: None,
                },
            ),
        };

        (status, Json(error_response)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    fn status_of(err: Error) -> StatusCode {
        err.into_response().status()
    }

    #[test]
    fn test_all_error_variants_status_codes() {
        assert_eq!(
            status_of(Error::NodeNotFound("x".into())),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            status_of(Error::ChildrenMissing(vec![])),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            status_of(Error::QuotaExceeded { used: 0, max: 0 }),
            StatusCode::INSUFFICIENT_STORAGE
        );
        assert_eq!(status_of(Error::BucketNotFound(1)), StatusCode::NOT_FOUND);
        assert_eq!(
            status_of(Error::RootNotFound("x".into())),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            status_of(Error::InvalidHash {
                expected: "a".into(),
                actual: "b".into()
            }),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(status_of(Error::InvalidSignature), StatusCode::BAD_REQUEST);
        assert_eq!(
            status_of(Error::NotAuthorized("x".into())),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            status_of(Error::Storage("x".into())),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            status_of(Error::Internal("x".into())),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            status_of(Error::Serialization("x".into())),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            status_of(Error::ObjectNotFound {
                bucket_id: 1,
                key: "k".into()
            }),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            status_of(Error::InvalidObjectKey("k".into())),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            status_of(Error::FileNotFound {
                bucket_id: 1,
                path: "/a".into()
            }),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            status_of(Error::NotAFile {
                bucket_id: 1,
                path: "/a".into()
            }),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            status_of(Error::InvalidPath("p".into())),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            status_of(AuthError::AuthRequired.into()),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            status_of(AuthError::TimestampExpired.into()),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            status_of(AuthError::InsufficientRole.into()),
            StatusCode::FORBIDDEN
        );
        // A chain blip is retryable; a decode failure is a bug. They must not
        // share a status code.
        assert_eq!(
            status_of(
                AuthError::MembershipLookup(MembershipError::Unavailable("down".into())).into()
            ),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            status_of(
                AuthError::MembershipLookup(MembershipError::Decode {
                    bucket_id: 1,
                    reason: "unexpected shape".into(),
                })
                .into()
            ),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            status_of(Error::SigningUnavailable),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            status_of(Error::NonceCounterUnavailable),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }

    #[test]
    fn test_error_response_json_structure() {
        let resp = Error::NodeNotFound("0xabc".into()).into_response();
        let (parts, body) = resp.into_parts();
        assert_eq!(parts.status, StatusCode::NOT_FOUND);

        let body_bytes = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async { axum::body::to_bytes(body, usize::MAX).await.unwrap() });
        let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(json["error"], "not_found");
        assert!(json.get("details").is_some());
        assert_eq!(json["details"]["hash"], "0xabc");
    }

    #[test]
    fn test_signing_unavailable_503() {
        let resp = Error::SigningUnavailable.into_response();
        let (parts, body) = resp.into_parts();
        assert_eq!(parts.status, StatusCode::SERVICE_UNAVAILABLE);

        let body_bytes = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async { axum::body::to_bytes(body, usize::MAX).await.unwrap() });
        let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(json["error"], "signing_unavailable");
        assert!(json["details"]["message"]
            .as_str()
            .unwrap()
            .contains("signer is not available."));
    }

    #[test]
    fn test_nonce_counter_unavailable_503() {
        let resp = Error::NonceCounterUnavailable.into_response();
        let (parts, body) = resp.into_parts();
        assert_eq!(parts.status, StatusCode::SERVICE_UNAVAILABLE);

        let body_bytes = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async { axum::body::to_bytes(body, usize::MAX).await.unwrap() });
        let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(json["error"], "nonce_counter_unavailable");
        assert!(json["details"]["message"]
            .as_str()
            .unwrap()
            .contains("nonce counter"));
    }
}
