// SPDX-License-Identifier: GPL-3.0-only

//! Error types for the provider node.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use provider_auth::{AuthError, MembershipError};
use provider_types::{ChainClientError, SigningRefused};
use serde::Serialize;
use std::fmt;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error(transparent)]
    Backend(#[from] provider_storage::Error),

    #[error("Invalid hash: expected {expected}, got {actual}")]
    InvalidHash { expected: String, actual: String },

    #[error("Invalid signature")]
    InvalidSignature,

    #[error("Not authorized: {0}")]
    NotAuthorized(String),

    /// A call against the chain failed: a state read, or an extrinsic that
    /// could not be submitted or was rejected.
    #[error(transparent)]
    ChainClient(#[from] ChainClientError),

    /// A value read from a request body or a peer response did not have the
    /// expected shape. Chain-state decode failures are
    /// [`ChainClientError::Decode`], which is not the caller's fault and does
    /// not share this status.
    #[error("Failed to decode {what}: {reason}")]
    Decode { what: &'static str, reason: String },

    /// The rate limiter itself failed (e.g. its backing store is
    /// unreachable). The request is rejected fail-closed rather than let
    /// through, but this is a distinct condition from [`Error::RateLimited`],
    /// which means the limiter ran and denied the request.
    #[error("Rate limiter failed: {0}")]
    RateLimiterFailed(String),

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

    #[error("Provider is deregistering; not accepting new agreements")]
    ProviderDeregistering,

    #[error(
        "Chain state not ready: current_anchor_block and request_timeout must both be non-zero"
    )]
    ChainStateNotReady,

    #[error(transparent)]
    Chain(#[from] provider_chain::Error),

    #[error(transparent)]
    Coordinator(#[from] provider_coordinator::Error),

    /// The node cannot sign with its registered key. Each reason keeps the
    /// response code it had when these were three separate variants.
    #[error(transparent)]
    Signing(#[from] provider_types::SigningRefused),

    #[error("Storage agreement requested 0 byte")]
    InvalidMaxBytesRequest,

    #[error("Too many requests")]
    RateLimited,
}

impl Error {
    /// A value read from a request body or a peer response did not decode
    /// into the expected shape.
    pub fn decode(what: &'static str, e: impl fmt::Display) -> Self {
        Error::Decode {
            what,
            reason: e.to_string(),
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
        use provider_chain::Error as ChainError;
        use provider_coordinator::Error as CoordinatorError;
        use provider_storage::Error as StorageError;
        let (status, error_response) = match &self {
            // Exhaustive on purpose — no wildcard — so a new storage variant
            // fails compilation until it gets an explicit status.
            Error::Backend(e) => match e {
                StorageError::NodeNotFound(hash) => (
                    StatusCode::NOT_FOUND,
                    ErrorResponse {
                        error: "not_found".to_string(),
                        details: Some(serde_json::json!({ "hash": hash })),
                    },
                ),
                StorageError::ResourceNotFound(resource) => (
                    StatusCode::NOT_FOUND,
                    ErrorResponse {
                        error: "not_found".to_string(),
                        details: Some(serde_json::json!({ "resource": resource })),
                    },
                ),
                StorageError::ChildrenMissing(children) => (
                    StatusCode::BAD_REQUEST,
                    ErrorResponse {
                        error: "children_missing".to_string(),
                        details: Some(serde_json::json!({ "missing": children })),
                    },
                ),
                StorageError::QuotaExceeded { used, max } => (
                    StatusCode::INSUFFICIENT_STORAGE,
                    ErrorResponse {
                        error: "quota_exceeded".to_string(),
                        details: Some(serde_json::json!({ "used": used, "max": max })),
                    },
                ),
                StorageError::BucketNotFound(id) => (
                    StatusCode::NOT_FOUND,
                    ErrorResponse {
                        error: "bucket_not_found".to_string(),
                        details: Some(serde_json::json!({ "bucket_id": id })),
                    },
                ),
                StorageError::RootNotFound(root) => (
                    StatusCode::NOT_FOUND,
                    ErrorResponse {
                        error: "root_not_found".to_string(),
                        details: Some(serde_json::json!({ "data_root": root })),
                    },
                ),
                StorageError::InvalidHash { expected, actual } => (
                    StatusCode::BAD_REQUEST,
                    ErrorResponse {
                        error: "invalid_hash".to_string(),
                        details: Some(serde_json::json!({
                            "expected": expected,
                            "actual": actual
                        })),
                    },
                ),
                e @ (StorageError::RocksDb(_) | StorageError::ColumnFamilyMissing(_)) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    ErrorResponse {
                        error: "internal_error".to_string(),
                        details: Some(serde_json::json!({ "message": e.to_string() })),
                    },
                ),
                StorageError::Serialization(msg) => (
                    StatusCode::BAD_REQUEST,
                    ErrorResponse {
                        error: "serialization_error".to_string(),
                        details: Some(serde_json::json!({ "message": msg })),
                    },
                ),
            },
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
            e @ Error::ChainClient(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorResponse {
                    error: "internal_error".to_string(),
                    details: Some(serde_json::json!({ "message": e.to_string() })),
                },
            ),
            // The reason is logged (see the rate-limit middleware) but kept out
            // of the response: it comes from the limiter's own backend and may
            // say more than an unauthenticated caller should learn.
            Error::RateLimiterFailed(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorResponse {
                    error: "internal_error".to_string(),
                    details: None,
                },
            ),
            e @ Error::Decode { .. } => (
                StatusCode::BAD_REQUEST,
                ErrorResponse {
                    error: "serialization_error".to_string(),
                    details: Some(serde_json::json!({ "message": e.to_string() })),
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
            // One variant, but each reason keeps the code and message it had
            // when these were three: a client tells "configure a key" from
            // "wait for the registration" by the `error` field.
            Error::Signing(refusal) => (
                StatusCode::SERVICE_UNAVAILABLE,
                match refusal {
                    SigningRefused::NoKey => ErrorResponse {
                        error: "signing_unavailable".to_string(),
                        details: Some(serde_json::json!({
                            "message": "provider node signer is not available."
                        })),
                    },
                    SigningRefused::Unregistered => ErrorResponse {
                        error: "provider_info_unavailable".to_string(),
                        details: Some(serde_json::json!({
                            "message": "provider's on-chain registration info is not loaded; \
                                        cannot validate agreement terms"
                        })),
                    },
                    SigningRefused::KeyMismatch => ErrorResponse {
                        error: "provider_key_mismatch".to_string(),
                        details: Some(serde_json::json!({
                            "message": "the node's signing key does not match the public_key \
                                        registered on-chain; signatures would never verify — \
                                        check --keyfile / --key-scheme against the registration"
                        })),
                    },
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
            // A chain error reaching us through the coordinator is the same
            // failure as a direct one, so both map to the same status.
            Error::Chain(err) | Error::Coordinator(CoordinatorError::Chain(err)) => match err {
                ChainError::NotConnected => (
                    StatusCode::SERVICE_UNAVAILABLE,
                    ErrorResponse {
                        error: "chain_unavailable".to_string(),
                        details: Some(serde_json::json!({
                            "message": "the node has not yet established a connection to the chain"
                        })),
                    },
                ),
                ChainError::Connection(_) | ChainError::Internal(_) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    ErrorResponse {
                        error: "internal_error".to_string(),
                        details: Some(serde_json::json!({ "message": err.to_string() })),
                    },
                ),
            },
            // Destructured rather than stringified: the inner message is already
            // the full text, so `to_string()` would prefix it a second time.
            Error::Coordinator(CoordinatorError::Internal(msg)) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorResponse {
                    error: "internal_error".to_string(),
                    details: Some(serde_json::json!({ "message": msg })),
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
    use sp_core::H256;

    fn status_of(err: Error) -> StatusCode {
        err.into_response().status()
    }

    #[test]
    fn test_all_error_variants_status_codes() {
        assert_eq!(
            status_of(Error::from(provider_storage::Error::NodeNotFound(
                H256::zero()
            ))),
            StatusCode::NOT_FOUND
        );
        // Storage-engine errors route through the transparent Backend variant.
        assert_eq!(
            status_of(Error::from(provider_storage::Error::ChildrenMissing(
                vec![]
            ))),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            status_of(Error::from(provider_storage::Error::QuotaExceeded {
                used: 0,
                max: 0
            })),
            StatusCode::INSUFFICIENT_STORAGE
        );
        assert_eq!(
            status_of(Error::from(provider_storage::Error::BucketNotFound(1))),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            status_of(Error::from(provider_storage::Error::RootNotFound(
                H256::zero()
            ))),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            status_of(Error::from(provider_storage::Error::ColumnFamilyMissing(
                "nodes"
            ))),
            StatusCode::INTERNAL_SERVER_ERROR
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
            status_of(ChainClientError::query("current block", "timed out").into()),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            status_of(ChainClientError::tx_submit("confirm_replica_sync", "watch dropped").into()),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            status_of(
                ChainClientError::tx_rejected("confirm_replica_sync", "SyncTooFrequent").into()
            ),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            status_of(Error::RateLimiterFailed("backend unreachable".into())),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            status_of(Error::decode("node data", "invalid base64")),
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
            status_of(SigningRefused::NoKey.into()),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            status_of(Error::NonceCounterUnavailable),
            StatusCode::SERVICE_UNAVAILABLE
        );
        // A connection that was never established is retryable; a failed
        // connect attempt is a bug. They must not share a status code.
        assert_eq!(
            status_of(provider_chain::Error::NotConnected.into()),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            status_of(provider_chain::Error::Internal("boom".into()).into()),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            status_of(provider_coordinator::Error::Internal("boom".into()).into()),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn test_error_response_json_structure() {
        let hash = H256::repeat_byte(0xab);
        let resp = Error::from(provider_storage::Error::NodeNotFound(hash)).into_response();
        let (parts, body) = resp.into_parts();
        assert_eq!(parts.status, StatusCode::NOT_FOUND);

        let body_bytes = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async { axum::body::to_bytes(body, usize::MAX).await.unwrap() });
        let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(json["error"], "not_found");
        assert!(json.get("details").is_some());
        // The storage engine holds an H256; the response renders it as hex.
        assert_eq!(
            json["details"]["hash"],
            "0xabababababababababababababababababababababababababababababababab"
        );
    }

    #[test]
    fn test_rate_limiter_failed_hides_detail_from_response() {
        let resp =
            Error::RateLimiterFailed("backend store unreachable".to_string()).into_response();
        let (parts, body) = resp.into_parts();
        assert_eq!(parts.status, StatusCode::INTERNAL_SERVER_ERROR);

        let body_bytes = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async { axum::body::to_bytes(body, usize::MAX).await.unwrap() });
        let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(json["error"], "internal_error");
        // The limiter's own failure reason must never reach the client - only
        // the tracing log (see the rate-limit middleware) carries it.
        assert!(json.get("details").is_none());
    }

    #[test]
    fn test_signing_unavailable_503() {
        let resp = Error::from(SigningRefused::NoKey).into_response();
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
    fn signing_refusals_keep_the_statuses_they_had() {
        assert_eq!(
            status_of(SigningRefused::NoKey.into()),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            status_of(SigningRefused::Unregistered.into()),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            status_of(SigningRefused::KeyMismatch.into()),
            StatusCode::SERVICE_UNAVAILABLE
        );
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
