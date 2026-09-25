// SPDX-License-Identifier: Apache-2.0

//! Error type for chain-connection handling.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Failed to connect to chain: {0}")]
    Connection(#[from] subxt::error::OnlineClientError),

    #[error("Chain connection not established yet")]
    NotConnected,

    #[error("Internal error: {0}")]
    Internal(String),
}
