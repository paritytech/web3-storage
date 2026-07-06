// SPDX-License-Identifier: Apache-2.0

//! Client error type.

use thiserror::Error;

/// Client errors.
#[derive(Error, Debug)]
pub enum ClientError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("API error: {0}")]
    Api(String),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Data verification failed")]
    VerificationFailed,

    #[error("Provider unavailable: {0}")]
    ProviderUnavailable(String),

    #[error("Chain error: {0}")]
    Chain(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Storage error: {0}")]
    Storage(String),
}

/// Result type for client operations.
pub type ClientResult<T> = Result<T, ClientError>;
