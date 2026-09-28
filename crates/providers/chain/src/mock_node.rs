// SPDX-License-Identifier: GPL-3.0-only

//! A mock node for tests that drive a real subxt `OnlineClient`: the handlers
//! every client needs to connect and to open a block-scoped handle, ready for
//! the per-test state and subscription handlers.

use subxt_rpcs::client::mock_rpc_client::{Json, MockRpcClientBuilder};
use subxt_rpcs::client::MockRpcClient;

/// Genesis hash the mock node reports.
pub const GENESIS_HASH: &str = "0x1111111111111111111111111111111111111111111111111111111111111111";
/// Finalized head the mock node reports; its header is block 42 on genesis.
pub const FINALIZED_HASH: &str =
    "0x2222222222222222222222222222222222222222222222222222222222222222";

/// A block header as the node serves it over JSON-RPC.
pub fn header_json(number: u32, parent_hash: &str) -> serde_json::Value {
    serde_json::json!({
        "parentHash": parent_hash,
        "number": format!("{number:#x}"),
        "stateRoot": GENESIS_HASH,
        "extrinsicsRoot": GENESIS_HASH,
        "digest": { "logs": [] }
    })
}

/// Builder with the metadata, runtime-version, genesis and finalized-head
/// handlers over the given SCALE-encoded runtime metadata. subxt reads the
/// metadata and the runtime version through `state_call`, whose functions are
/// runtime APIs named `Trait_method` (`Core_version`, `Metadata_metadata`);
/// `runtime_calls` answers the ones a test needs beyond those, as SCALE
/// bytes, and `None` panics the mock.
pub fn mock_node(
    metadata: &'static [u8],
    runtime_calls: impl Fn(&str) -> Option<Vec<u8>> + Clone + Send + Sync + 'static,
) -> MockRpcClientBuilder {
    let metadata_hex = format!("0x{}", hex::encode(metadata));
    MockRpcClient::builder()
        .method_handler("state_getMetadata", move |_params| {
            let metadata_hex = metadata_hex.clone();
            async move { Json(metadata_hex) }
        })
        .method_handler("state_call", move |params| {
            let runtime_calls = runtime_calls.clone();
            async move {
                use codec::Encode;
                let raw = params.map(|p| p.get().to_string()).unwrap_or_default();
                let function: String = serde_json::from_str::<Vec<serde_json::Value>>(&raw)
                    .ok()
                    .and_then(|p| p.first().and_then(|f| f.as_str().map(str::to_string)))
                    .unwrap_or_default();
                let response = match function.as_str() {
                    "Metadata_metadata_versions" => vec![u32::from(metadata[4])].encode(),
                    "Metadata_metadata_at_version" => Some(metadata.to_vec()).encode(),
                    "Metadata_metadata" => metadata.to_vec().encode(),
                    "Core_version" => (
                        "test".to_string(),
                        "test".to_string(),
                        1u32,
                        1u32,
                        1u32,
                        Vec::<([u8; 8], u32)>::new(),
                        1u32,
                        1u8,
                    )
                        .encode(),
                    other => runtime_calls(other)
                        .unwrap_or_else(|| panic!("mock RPC: unhandled state_call {other}")),
                };
                Json(format!("0x{}", hex::encode(response)))
            }
        })
        .method_handler("chain_getBlockHash", |_params| async {
            Json(GENESIS_HASH.to_string())
        })
        .method_handler("chain_getFinalizedHead", |_params| async {
            Json(FINALIZED_HASH.to_string())
        })
        .method_handler("chain_getHeader", |_params| async {
            Json(header_json(42, GENESIS_HASH))
        })
        .method_handler("state_getRuntimeVersion", |_params| async {
            Json(runtime_version_json())
        })
}

/// The `state_getRuntimeVersion` answer of the mock node.
pub fn runtime_version_json() -> serde_json::Value {
    serde_json::json!({
        "specName": "test",
        "implName": "test",
        "authoringVersion": 1,
        "specVersion": 1,
        "implVersion": 1,
        "apis": [],
        "transactionVersion": 1,
        "stateVersion": 1
    })
}
