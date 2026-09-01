// SPDX-License-Identifier: Apache-2.0

//! Chain-connection construction: the single place a subxt client is built.
//!
//! Every chain consumer in the provider node (the chain-state follower, the
//! shared signing client, the auth membership resolver) receives the current
//! connection through a [`tokio::sync::watch`] channel of [`ChainHandle`]s.
//! The chain-state coordinator owns the sender: it (re)builds the connection
//! in its reconnect loop and publishes each new handle, so consumers always
//! borrow the live client and nobody else carries reconnect logic.

use crate::Error;
use serde_json::{json, Value};
use std::path::PathBuf;
use subxt::lightclient::LightClient;
use subxt::{OnlineClient, PolkadotConfig};
use subxt_rpcs::client::{rpc_params, RpcClient};

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
    LightClient {
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
    /// Build the spec by querying a running node's RPC at startup: the relay
    /// spec via the sync-state RPC, the parachain spec assembled from
    /// ordinary RPC calls (genesis state root, boot-node addresses, para id).
    /// Convenient for local development (zombienet regenerates genesis every
    /// run), but it reintroduces trust in that node — dev use only.
    FetchFromRpc(String),
}

impl SpecSource {
    /// Load the relay-chain spec.
    async fn load_relay(&self) -> Result<String, Error> {
        match self {
            SpecSource::File(path) => read_spec_file(path, "relay"),
            SpecSource::FetchFromRpc(url) => {
                warn_fetch_trust("relay", url);
                fetch_relay_spec(&connect_rpc(url, "relay").await?, url).await
            }
        }
    }

    /// Load the parachain spec. `relay_spec` is the already-loaded relay
    /// spec: an assembled parachain spec must name that spec's `id` as its
    /// `relay_chain` for smoldot to match the two.
    async fn load_para(&self, relay_spec: &str) -> Result<String, Error> {
        match self {
            SpecSource::File(path) => read_spec_file(path, "parachain"),
            SpecSource::FetchFromRpc(url) => {
                warn_fetch_trust("parachain", url);
                let relay_id = spec_id(relay_spec)?;
                fetch_para_spec(&connect_rpc(url, "parachain").await?, url, &relay_id).await
            }
        }
    }
}

/// `ParachainInfo::ParachainId` storage key:
/// `twox128("ParachainInfo") ++ twox128("ParachainId")`.
const PARA_ID_STORAGE_KEY: &str =
    "0x0d715f2646c8f85767b5d2764bb2782604a74d81251e398fd8a0a4d55023bb3f";

fn read_spec_file(path: &PathBuf, what: &str) -> Result<String, Error> {
    std::fs::read_to_string(path).map_err(|e| {
        Error::Internal(format!(
            "Failed to read {what} chain spec {}: {e}",
            path.display()
        ))
    })
}

fn warn_fetch_trust(what: &str, url: &str) {
    tracing::warn!(
        "Fetching the {what} chain spec from {url}: this trusts that node and defeats the \
         light client's verification purpose — use spec files in production"
    );
}

fn fetch_err(what: &str, url: &str, e: impl std::fmt::Display) -> Error {
    Error::Internal(format!("Failed to fetch {what} chain spec from {url}: {e}"))
}

/// The fetch paths are dev-only and the URL comes from the operator's CLI, so
/// plain `ws://` endpoints like zombienet's are allowed.
async fn connect_rpc(url: &str, what: &str) -> Result<RpcClient, Error> {
    RpcClient::from_insecure_url(url)
        .await
        .map_err(|e| fetch_err(what, url, e))
}

/// Light-client-ready relay spec from the node: `sync_state_genSyncSpec`
/// (the node's spec with raw genesis — smoldot needs the genesis state for
/// the GRANDPA authorities), with the node's own address injected as a boot
/// node when the generated spec has none (generated sync specs never list
/// any).
async fn fetch_relay_spec(client: &RpcClient, url: &str) -> Result<String, Error> {
    let mut spec: Value = client
        .request("sync_state_genSyncSpec", rpc_params![true])
        .await
        .map_err(|e| {
            fetch_err(
                "relay",
                url,
                format!("{e} (the node must expose the sync-state RPC)"),
            )
        })?;
    // Generated sync specs embed a lightSyncState checkpoint. Drop it: vetted
    // spec files don't carry one, and a checkpoint start leaves smoldot
    // waiting minutes for the next GrandPa commit on quiet dev relays, while
    // genesis warp sync tracks finality immediately.
    if let Some(obj) = spec.as_object_mut() {
        obj.remove("lightSyncState");
    }
    if !has_boot_nodes(&spec) {
        spec["bootNodes"] = json!(fetch_own_boot_nodes(client, url, "relay").await?);
    }
    Ok(spec.to_string())
}

fn has_boot_nodes(spec: &Value) -> bool {
    spec.get("bootNodes")
        .and_then(Value::as_array)
        .is_some_and(|b| !b.is_empty())
}

/// Assemble a minimal parachain spec from ordinary RPC calls. Omni-node has
/// no sync-state RPC, and a parachain spec needs no genesis state or
/// checkpoint anyway: smoldot derives the para head from the relay chain, so
/// the genesis state root alone identifies the chain.
async fn fetch_para_spec(client: &RpcClient, url: &str, relay_id: &str) -> Result<String, Error> {
    let name: String = client
        .request("system_chain", rpc_params![])
        .await
        .map_err(|e| fetch_err("parachain", url, e))?;
    let chain_type: Value = client
        .request("system_chainType", rpc_params![])
        .await
        .map_err(|e| fetch_err("parachain", url, e))?;
    let properties: Value = client
        .request("system_properties", rpc_params![])
        .await
        .map_err(|e| fetch_err("parachain", url, e))?;
    let genesis_hash: String = client
        .request("chain_getBlockHash", rpc_params![0u32])
        .await
        .map_err(|e| fetch_err("parachain", url, e))?;
    let header: Value = client
        .request("chain_getHeader", rpc_params![&genesis_hash])
        .await
        .map_err(|e| fetch_err("parachain", url, e))?;
    let state_root = header
        .get("stateRoot")
        .and_then(Value::as_str)
        .ok_or_else(|| fetch_err("parachain", url, "genesis header has no stateRoot"))?
        .to_owned();
    let raw_para_id: Option<String> = client
        .request("state_getStorage", rpc_params![PARA_ID_STORAGE_KEY])
        .await
        .map_err(|e| fetch_err("parachain", url, e))?;
    let para_id = raw_para_id
        .as_deref()
        .and_then(decode_para_id)
        .ok_or_else(|| {
            fetch_err(
                "parachain",
                url,
                "could not read ParachainInfo::ParachainId — is this a parachain node?",
            )
        })?;
    let boot_nodes = fetch_own_boot_nodes(client, url, "parachain").await?;
    Ok(build_para_spec(
        &name,
        chain_type,
        relay_id,
        para_id,
        boot_nodes,
        properties,
        &state_root,
    ))
}

/// The node's own dialable addresses, for use as boot nodes.
async fn fetch_own_boot_nodes(
    client: &RpcClient,
    url: &str,
    what: &str,
) -> Result<Vec<String>, Error> {
    let peer_id: String = client
        .request("system_localPeerId", rpc_params![])
        .await
        .map_err(|e| fetch_err(what, url, e))?;
    let listen: Vec<String> = client
        .request("system_localListenAddresses", rpc_params![])
        .await
        .map_err(|e| fetch_err(what, url, e))?;
    let boot = boot_node_addrs(&listen, &peer_id);
    if boot.is_empty() {
        return Err(fetch_err(
            what,
            url,
            "the node reports no TCP listen addresses to use as boot nodes",
        ));
    }
    Ok(boot)
}

/// TCP-based listen addresses (raw TCP and WS — both dialable by native
/// smoldot) with any `/p2p/…` tail replaced by the canonical peer id (nodes
/// report it doubled when started with a `/p2p/`-suffixed public address),
/// deduplicated.
fn boot_node_addrs(listen: &[String], peer_id: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for addr in listen {
        if !addr.contains("/tcp/") {
            continue;
        }
        let base = match addr.split_once("/p2p/") {
            Some((base, _)) => base,
            None => addr.as_str(),
        };
        let full = format!("{base}/p2p/{peer_id}");
        if !out.contains(&full) {
            out.push(full);
        }
    }
    out
}

/// A minimal parachain spec: identity + boot nodes + genesis state root.
fn build_para_spec(
    name: &str,
    chain_type: Value,
    relay_id: &str,
    para_id: u32,
    boot_nodes: Vec<String>,
    properties: Value,
    state_root: &str,
) -> String {
    json!({
        "id": name.to_lowercase().replace(' ', "-"),
        "name": name,
        "chainType": chain_type,
        "relay_chain": relay_id,
        "para_id": para_id,
        "bootNodes": boot_nodes,
        "properties": properties,
        "genesis": { "stateRootHash": state_root },
    })
    .to_string()
}

/// SCALE-encoded `ParaId` storage value (e.g. `"0x40060000"`) → `u32`.
fn decode_para_id(hex_value: &str) -> Option<u32> {
    let bytes = hex::decode(hex_value.strip_prefix("0x")?).ok()?;
    Some(u32::from_le_bytes(bytes.try_into().ok()?))
}

/// The `id` field of a chain spec.
fn spec_id(spec: &str) -> Result<String, Error> {
    serde_json::from_str::<Value>(spec)
        .ok()
        .and_then(|v| v.get("id").and_then(Value::as_str).map(str::to_owned))
        .ok_or_else(|| {
            Error::Internal(
                "Relay chain spec has no \"id\" field to bind the fetched parachain spec to"
                    .to_string(),
            )
        })
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
    pub fn from_api(api: OnlineClient<PolkadotConfig>) -> Self {
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
            let api = OnlineClient::<PolkadotConfig>::from_url(url).await?;
            Ok(ChainHandle::from_api(api))
        }
        ChainTransport::LightClient {
            relay_spec,
            para_spec,
        } => {
            let relay = relay_spec.load_relay().await?;
            let para = para_spec.load_para(&relay).await?;

            let (light_client, _relay_rpc) = LightClient::relay_chain(relay.as_str())
                .map_err(|e| Error::Internal(format!("Failed to start light client: {e}")))?;
            let para_rpc = light_client.parachain(para.as_str()).map_err(|e| {
                Error::Internal(format!("Failed to add parachain to light client: {e}"))
            })?;
            let api = OnlineClient::<PolkadotConfig>::from_rpc_client(para_rpc)
                .await
                .map_err(|e| Error::Internal(format!("Failed to connect via light client: {e}")))?;

            // The client is usable before smoldot has observed relay finality
            // (after a checkpoint start, the first GrandPa commit can take
            // minutes to arrive). Wait for one finalized block here, under the
            // caller's generous connect budget, so the chain-state follower's
            // tight bootstrap budget starts on a synced connection — rebuilding
            // a not-yet-synced light client only resets its sync progress.
            let mut blocks = api.stream_blocks().await.map_err(|e| {
                Error::Internal(format!("Light client block subscription failed: {e}"))
            })?;
            blocks
                .next()
                .await
                .ok_or_else(|| {
                    Error::Internal("Light client block stream ended during sync".to_string())
                })?
                .map_err(|e| {
                    Error::Internal(format!("Light client first finalized block failed: {e}"))
                })?;

            tracing::info!("Embedded light client started (relay + parachain)");
            Ok(ChainHandle {
                api,
                _light: Some(light_client),
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
        .ok_or(Error::NotConnected)
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
        assert!(
            matches!(err, Error::Connection(_)),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn current_api_errors_before_first_connect() {
        let (_tx, rx) = tokio::sync::watch::channel(None);
        let err = current_api(&rx).expect_err("no connection published yet");
        assert!(
            matches!(err, Error::NotConnected),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn light_with_missing_spec_file_errors() {
        let err = connect(&ChainTransport::LightClient {
            relay_spec: SpecSource::File(PathBuf::from("/nonexistent/relay.json")),
            para_spec: SpecSource::File(PathBuf::from("/nonexistent/para.json")),
        })
        .await
        .map(|_| ())
        .expect_err("connect must fail when the relay spec file is missing");
        assert!(
            err.to_string().contains("Failed to read relay chain spec"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn light_with_invalid_spec_errors() {
        // The file loads (covering the File happy path) but smoldot rejects
        // the contents as a chain spec.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("relay.json");
        std::fs::write(&path, "{\"not\": \"a chain spec\"}").unwrap();

        let err = connect(&ChainTransport::LightClient {
            relay_spec: SpecSource::File(path),
            para_spec: SpecSource::File(PathBuf::from("/nonexistent/para.json")),
        })
        .await
        .map(|_| ())
        .expect_err("connect must fail on an invalid relay spec");
        // The para spec is never reached: either the relay spec parse fails,
        // or (if smoldot were lenient) the missing para file errors next.
        assert!(
            err.to_string().contains("Failed to start light client")
                || err
                    .to_string()
                    .contains("Failed to read parachain chain spec"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn light_spec_fetch_from_unreachable_node_errors() {
        let err = connect(&ChainTransport::LightClient {
            relay_spec: SpecSource::FetchFromRpc("ws://127.0.0.1:1".to_string()),
            para_spec: SpecSource::File(PathBuf::from("/nonexistent/para.json")),
        })
        .await
        .map(|_| ())
        .expect_err("connect must fail when the spec fetch node is unreachable");
        assert!(
            err.to_string().contains("Failed to fetch relay chain spec"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn light_para_fetch_from_unreachable_node_errors() {
        // The relay file loads and its id parses; the para fetch then fails.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("relay.json");
        std::fs::write(&path, "{\"id\": \"paseo-local\"}").unwrap();

        let err = connect(&ChainTransport::LightClient {
            relay_spec: SpecSource::File(path),
            para_spec: SpecSource::FetchFromRpc("ws://127.0.0.1:1".to_string()),
        })
        .await
        .map(|_| ())
        .expect_err("connect must fail when the para spec fetch node is unreachable");
        assert!(
            err.to_string()
                .contains("Failed to fetch parachain chain spec"),
            "unexpected error: {err}"
        );
    }

    mod spec_fetch {
        use super::*;
        use subxt_rpcs::client::mock_rpc_client::{Json, MockRpcClient};

        /// Mock of what a chain node answers to the spec-assembly RPC calls.
        /// `genesis_header` and `para_id_storage` are raw JSON results so
        /// tests can hand back malformed values.
        fn para_node_mock(genesis_header: Value, para_id_storage: Value) -> RpcClient {
            let mock = MockRpcClient::builder()
                .method_handler("system_chain", |_| async { Json("Web3 Storage Local") })
                .method_handler("system_chainType", |_| async { Json("Local") })
                .method_handler("system_properties", |_| async {
                    Json(json!({"tokenSymbol": "PAS", "tokenDecimals": 10}))
                })
                .method_handler("chain_getBlockHash", |_| async { Json("0xgenesis") })
                .method_handler("chain_getHeader", move |_| {
                    let header = genesis_header.clone();
                    async move { header }
                })
                .method_handler("state_getStorage", move |_| {
                    let stored = para_id_storage.clone();
                    async move { stored }
                })
                .method_handler("system_localPeerId", |_| async { Json("12D3KooWTest") })
                .method_handler("system_localListenAddresses", |_| async {
                    Json(json!([
                        // Doubled /p2p/ tail, as nodes report when started
                        // with a /p2p/-suffixed public address.
                        "/ip4/127.0.0.1/tcp/30333/ws/p2p/12D3KooWTest/p2p/12D3KooWTest",
                        // Not TCP-dialable by native smoldot: must be dropped.
                        "/ip4/10.0.0.1/udp/30333/webrtc-direct/certhash/uEiA",
                    ]))
                })
                .build();
            RpcClient::new(mock)
        }

        #[tokio::test]
        async fn para_fetch_assembles_minimal_spec() {
            let client = para_node_mock(
                json!({"stateRoot": "0x50a1"}),
                json!("0x40060000"), // 1600 little-endian
            );
            let spec = fetch_para_spec(&client, "ws://mock", "paseo-local")
                .await
                .expect("assembly succeeds");
            let spec: Value = serde_json::from_str(&spec).unwrap();
            assert_eq!(spec["id"], "web3-storage-local");
            assert_eq!(spec["name"], "Web3 Storage Local");
            assert_eq!(spec["relay_chain"], "paseo-local");
            assert_eq!(spec["para_id"], 1600);
            assert_eq!(spec["genesis"]["stateRootHash"], "0x50a1");
            assert_eq!(
                spec["bootNodes"],
                json!(["/ip4/127.0.0.1/tcp/30333/ws/p2p/12D3KooWTest"])
            );
            assert_eq!(spec["properties"]["tokenSymbol"], "PAS");
        }

        #[tokio::test]
        async fn para_fetch_without_parachain_info_errors() {
            // A relay node answers all the same RPCs but has no
            // ParachainInfo::ParachainId in storage.
            let client = para_node_mock(json!({"stateRoot": "0x50a1"}), Value::Null);
            let err = fetch_para_spec(&client, "ws://mock", "paseo-local")
                .await
                .expect_err("must refuse to build a para spec without a para id");
            assert!(
                err.to_string().contains("ParachainInfo::ParachainId"),
                "unexpected error: {err}"
            );
        }

        #[tokio::test]
        async fn relay_fetch_without_sync_state_rpc_errors() {
            // Omni-node behavior: no handlers registered, so every method
            // (including sync_state_genSyncSpec) returns "method not found".
            let client = RpcClient::new(MockRpcClient::builder().build());
            let err = fetch_relay_spec(&client, "ws://mock")
                .await
                .expect_err("must fail without the sync-state RPC");
            assert!(
                err.to_string().contains("must expose the sync-state RPC"),
                "unexpected error: {err}"
            );
        }

        #[tokio::test]
        async fn relay_fetch_injects_boot_nodes_and_drops_checkpoint() {
            let mock = MockRpcClient::builder()
                .method_handler("sync_state_genSyncSpec", |_| async {
                    Json(json!({
                        "id": "paseo-local",
                        "bootNodes": [],
                        "lightSyncState": {"finalizedBlockHeader": "0x..."},
                    }))
                })
                .method_handler("system_localPeerId", |_| async { Json("12D3KooWRelay") })
                .method_handler("system_localListenAddresses", |_| async {
                    Json(json!(["/ip4/127.0.0.1/tcp/30334/ws"]))
                })
                .build();
            let spec = fetch_relay_spec(&RpcClient::new(mock), "ws://mock")
                .await
                .expect("fetch succeeds");
            let spec: Value = serde_json::from_str(&spec).unwrap();
            assert_eq!(
                spec["bootNodes"],
                json!(["/ip4/127.0.0.1/tcp/30334/ws/p2p/12D3KooWRelay"])
            );
            assert!(
                spec.get("lightSyncState").is_none(),
                "checkpoint must be dropped"
            );
        }

        #[tokio::test]
        async fn relay_fetch_keeps_existing_boot_nodes() {
            // No system_local* handlers: reaching for them would error, so
            // this also proves they are not queried when not needed.
            let mock = MockRpcClient::builder()
                .method_handler("sync_state_genSyncSpec", |_| async {
                    Json(json!({"id": "paseo-local", "bootNodes": ["/dns/x/tcp/1/p2p/12D3"]}))
                })
                .build();
            let spec = fetch_relay_spec(&RpcClient::new(mock), "ws://mock")
                .await
                .expect("fetch succeeds");
            let spec: Value = serde_json::from_str(&spec).unwrap();
            assert_eq!(spec["bootNodes"], json!(["/dns/x/tcp/1/p2p/12D3"]));
        }

        #[test]
        fn boot_node_addrs_normalizes_filters_and_dedups() {
            let listen = [
                "/ip4/127.0.0.1/tcp/1/ws/p2p/OLD/p2p/OLD".to_string(),
                "/ip4/127.0.0.1/tcp/1/ws".to_string(), // dedups with the above
                "/ip4/10.0.0.1/udp/1/webrtc-direct/certhash/uEiA".to_string(),
                "/ip4/9.9.9.9/tcp/2".to_string(),
            ];
            assert_eq!(
                boot_node_addrs(&listen, "PEER"),
                vec![
                    "/ip4/127.0.0.1/tcp/1/ws/p2p/PEER".to_string(),
                    "/ip4/9.9.9.9/tcp/2/p2p/PEER".to_string(),
                ]
            );
        }

        #[test]
        fn decode_para_id_reads_little_endian() {
            assert_eq!(decode_para_id("0x40060000"), Some(1600));
            assert_eq!(decode_para_id("0xa00f0000"), Some(4000));
            assert_eq!(decode_para_id("40060000"), None, "0x prefix required");
            assert_eq!(decode_para_id("0x4006"), None, "must be 4 bytes");
            assert_eq!(decode_para_id("0xzz060000"), None, "must be hex");
        }

        #[test]
        fn spec_id_requires_an_id_field() {
            assert_eq!(spec_id("{\"id\": \"paseo-local\"}").unwrap(), "paseo-local");
            assert!(spec_id("{}").is_err());
            assert!(spec_id("not json").is_err());
        }
    }
}
