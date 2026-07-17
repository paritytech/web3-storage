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
use std::path::PathBuf;
use subxt::lightclient::LightClient;
use subxt::{OnlineClient, PolkadotConfig};

/// How the provider node talks to the chain.
#[derive(Clone, Debug)]
pub enum ChainTransport {
    /// External RPC node reached over WebSocket.
    Rpc {
        /// `ws://` / `wss://` URL of the parachain RPC endpoint.
        url: String,
    },
    /// Embedded smoldot light client following the relay chain and deriving
    /// parachain finality from it — no operated RPC infrastructure needed.
    Light {
        /// Relay-chain spec (with reachable boot nodes).
        relay_spec: SpecSource,
        /// Parachain spec (with boot nodes serving the light-client
        /// request-response protocols).
        para_spec: SpecSource,
    },
}

/// Where a chain spec for the light client comes from.
#[derive(Clone, Debug)]
pub enum SpecSource {
    /// A chain-spec JSON file shipped with the deployment. This is the
    /// trust-preserving option: the spec (genesis + boot nodes) is vetted
    /// ahead of time rather than trusted from a node at runtime.
    File(PathBuf),
    /// Fetch the spec from a running node's RPC at startup. Convenient for
    /// local development (zombienet regenerates genesis every run), but it
    /// reintroduces trust in that node — dev use only.
    FetchFromRpc(String),
}

impl SpecSource {
    async fn load(&self, what: &str) -> Result<String, Error> {
        match self {
            SpecSource::File(path) => std::fs::read_to_string(path).map_err(|e| {
                Error::Internal(format!(
                    "Failed to read {what} chain spec {}: {e}",
                    path.display()
                ))
            }),
            SpecSource::FetchFromRpc(url) => {
                tracing::warn!(
                    "Fetching the {what} chain spec from {url}: this trusts that node and \
                     defeats the light client's verification purpose — use spec files in \
                     production"
                );
                let spec = subxt::utils::fetch_chainspec_from_rpc_node(url)
                    .await
                    .map_err(|e| {
                        Error::Internal(format!(
                            "Failed to fetch {what} chain spec from {url}: {e}"
                        ))
                    })?;
                Ok(spec.get().to_string())
            }
        }
    }
}

/// A live chain connection, cheap to clone (`OnlineClient` is `Arc`-backed).
#[derive(Clone)]
pub struct ChainHandle {
    /// The subxt client for storage reads, event decoding, and tx submission.
    pub api: OnlineClient<PolkadotConfig>,
    /// Keeps the embedded smoldot instance alive for the handle's lifetime;
    /// dropping the last clone tears the light client down. `None` on the
    /// RPC transport.
    _light: Option<LightClient>,
}

impl ChainHandle {
    /// Handle over an existing client with no embedded light client to keep
    /// alive: the RPC transport, and tests driving a mock connection.
    pub(crate) fn from_api(api: OnlineClient<PolkadotConfig>) -> Self {
        Self { api, _light: None }
    }
}

/// Receiver side of the connection watch channel. `None` until the first
/// successful connect.
pub type ChainWatch = tokio::sync::watch::Receiver<Option<ChainHandle>>;

/// Build a fresh connection for the given transport.
///
/// For `Light` this boots a new embedded smoldot instance (relay + para);
/// the watchdog's rebuild path therefore recovers even from a wedged smoldot
/// background task, not just a dropped subscription.
pub async fn connect(transport: &ChainTransport) -> Result<ChainHandle, Error> {
    match transport {
        ChainTransport::Rpc { url } => {
            let api = OnlineClient::<PolkadotConfig>::from_url(url)
                .await
                .map_err(|e| Error::Internal(format!("Failed to connect to chain: {e}")))?;
            Ok(ChainHandle::from_api(api))
        }
        ChainTransport::Light {
            relay_spec,
            para_spec,
        } => {
            let relay = relay_spec.load("relay").await?;
            let para = para_spec.load("parachain").await?;

            let (light, _relay_rpc) = LightClient::relay_chain(relay.as_str())
                .map_err(|e| Error::Internal(format!("Failed to start light client: {e}")))?;
            let para_rpc = light.parachain(para.as_str()).map_err(|e| {
                Error::Internal(format!("Failed to add parachain to light client: {e}"))
            })?;
            let api = OnlineClient::<PolkadotConfig>::from_rpc_client(para_rpc)
                .await
                .map_err(|e| Error::Internal(format!("Failed to connect via light client: {e}")))?;

            tracing::info!("Embedded light client started (relay + parachain)");
            Ok(ChainHandle {
                api,
                _light: Some(light),
            })
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
