//! Chain client construction.
//!
//! Building the `OnlineClient<PolkadotConfig>` happens here, behind a single helper, so
//! that the transport choice (WebSocket RPC vs. embedded smoldot light client) is made in
//! exactly one place. Coordinators receive a clone of the constructed client.

use crate::error::Error;
use std::path::PathBuf;
use subxt::{OnlineClient, PolkadotConfig};

/// How to talk to the parachain.
#[derive(Clone, Debug)]
pub enum ChainTransport {
    /// Standard JSON-RPC over WebSocket against a full node.
    WebSocket(String),
    /// Embedded smoldot light client. Only available when the `light-client` feature is
    /// enabled at build time.
    #[cfg(feature = "light-client")]
    LightClient {
        relay_spec_path: PathBuf,
        parachain_spec_path: PathBuf,
    },
}

impl ChainTransport {
    /// Short label for logs.
    pub fn label(&self) -> String {
        match self {
            ChainTransport::WebSocket(url) => format!("WebSocket({url})"),
            #[cfg(feature = "light-client")]
            ChainTransport::LightClient { .. } => "LightClient(smoldot)".to_string(),
        }
    }
}

/// Connect using the chosen transport.
pub async fn connect(transport: &ChainTransport) -> Result<OnlineClient<PolkadotConfig>, Error> {
    match transport {
        ChainTransport::WebSocket(url) => OnlineClient::<PolkadotConfig>::from_url(url)
            .await
            .map_err(|e| Error::Internal(format!("WebSocket connect to {url} failed: {e}"))),
        #[cfg(feature = "light-client")]
        ChainTransport::LightClient {
            relay_spec_path,
            parachain_spec_path,
        } => connect_light_client(relay_spec_path, parachain_spec_path).await,
    }
}

#[cfg(feature = "light-client")]
async fn connect_light_client(
    relay_spec_path: &std::path::Path,
    parachain_spec_path: &std::path::Path,
) -> Result<OnlineClient<PolkadotConfig>, Error> {
    let relay_spec = std::fs::read_to_string(relay_spec_path).map_err(|e| {
        Error::Internal(format!(
            "Failed to read relay chain spec {}: {e}",
            relay_spec_path.display()
        ))
    })?;
    let para_spec = std::fs::read_to_string(parachain_spec_path).map_err(|e| {
        Error::Internal(format!(
            "Failed to read parachain spec {}: {e}",
            parachain_spec_path.display()
        ))
    })?;

    tracing::info!(
        "Starting smoldot light client (relay={}, parachain={})",
        relay_spec_path.display(),
        parachain_spec_path.display()
    );

    let (lc, _relay_rpc) = subxt::lightclient::LightClient::relay_chain(relay_spec.as_str())
        .map_err(|e| Error::Internal(format!("Light client relay chain init failed: {e}")))?;
    let para_rpc = lc
        .parachain(para_spec.as_str())
        .map_err(|e| Error::Internal(format!("Light client parachain init failed: {e}")))?;

    OnlineClient::<PolkadotConfig>::from_rpc_client(para_rpc)
        .await
        .map_err(|e| Error::Internal(format!("OnlineClient::from_rpc_client (LC) failed: {e}")))
}

/// Build a `ChainTransport` from CLI flags. Returns an error if light-client flags are
/// used but the `light-client` feature wasn't built in, or if required spec paths are
/// missing.
pub fn transport_from_cli(
    chain_rpc: &str,
    light_client: bool,
    relay_spec: Option<&PathBuf>,
    parachain_spec: Option<&PathBuf>,
) -> Result<ChainTransport, String> {
    if !light_client {
        return Ok(ChainTransport::WebSocket(chain_rpc.to_string()));
    }

    #[cfg(feature = "light-client")]
    {
        match (relay_spec, parachain_spec) {
            (Some(relay), Some(para)) => Ok(ChainTransport::LightClient {
                relay_spec_path: relay.clone(),
                parachain_spec_path: para.clone(),
            }),
            _ => Err(
                "--chain-light-client requires both --chain-relay-spec and --chain-parachain-spec"
                    .to_string(),
            ),
        }
    }
    #[cfg(not(feature = "light-client"))]
    {
        let _ = (relay_spec, parachain_spec);
        Err("--chain-light-client requires building with `--features light-client`".to_string())
    }
}
