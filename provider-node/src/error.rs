//! Error types for the provider node.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
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
        };

        (status, Json(error_response)).into_response()
    }
}
