// SPDX-License-Identifier: GPL-3.0-only

//! Chain-connection construction: the single place a subxt client is built.
//!
//! Every chain consumer in the provider node (the chain-state follower, the
//! shared signing client, the auth membership resolver) receives the current
//! connection through a [`tokio::sync::watch`] channel of [`ChainHandle`]s.
//! The chain-state coordinator owns the sender: it (re)builds the connection
//! in its reconnect loop and publishes each new handle, so consumers always
//! borrow the live client and nobody else carries reconnect logic.

use crate::Error;
use subxt::{OnlineClient, PolkadotConfig};

/// How the provider node talks to the chain.
///
/// A `Light` (embedded smoldot) variant is planned as a follow-up; keeping
/// construction behind this enum means adding it only touches this module.
#[derive(Clone, Debug)]
pub enum ChainTransport {
    /// External RPC node reached over WebSocket.
    Rpc {
        /// `ws://` / `wss://` URL of the parachain RPC endpoint.
        url: String,
    },
}

/// A live chain connection, cheap to clone (`OnlineClient` is `Arc`-backed).
#[derive(Clone)]
pub struct ChainHandle {
    /// The subxt client for storage reads, event decoding, and tx submission.
    pub api: OnlineClient<PolkadotConfig>,
}

/// Receiver side of the connection watch channel. `None` until the first
/// successful connect.
pub type ChainWatch = tokio::sync::watch::Receiver<Option<ChainHandle>>;

/// Build a fresh connection for the given transport.
pub async fn connect(transport: &ChainTransport) -> Result<ChainHandle, Error> {
    match transport {
        ChainTransport::Rpc { url } => {
            let api = OnlineClient::<PolkadotConfig>::from_url(url)
                .await
                .map_err(|e| Error::Internal(format!("Failed to connect to chain: {e}")))?;
            Ok(ChainHandle { api })
        }
    }
}

/// Borrow the current connection from a watch receiver, or fail if the chain
/// has never been reached yet.
pub fn current_api(chain_rx: &ChainWatch) -> Result<OnlineClient<PolkadotConfig>, Error> {
    chain_rx
        .borrow()
        .as_ref()
        .map(|h| h.api.clone())
        .ok_or_else(|| Error::Internal("Chain connection not established yet".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rpc_connect_to_unreachable_chain_errors() {
        // Port 1 on loopback refuses immediately.
        let result = connect(&ChainTransport::Rpc {
            url: "ws://127.0.0.1:1".to_string(),
        })
        .await;
        let Err(err) = result else {
            panic!("connect must fail against a closed port");
        };
        assert!(err.to_string().contains("Failed to connect to chain"));
    }

    #[test]
    fn current_api_errors_before_first_connect() {
        let (_tx, rx) = tokio::sync::watch::channel(None);
        let err = current_api(&rx).expect_err("no connection published yet");
        assert!(err.to_string().contains("not established"));
    }
}
