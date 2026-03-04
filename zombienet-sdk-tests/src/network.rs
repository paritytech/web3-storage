//! Network configuration builders and spawning utilities.

use crate::config::*;
use anyhow::{anyhow, Result};
use zombienet_sdk::{
    LocalFileSystem, Network, NetworkConfig, NetworkConfigBuilder, NetworkConfigExt,
};

/// Build a network configuration with relay chain + parachain.
pub fn build_network_config() -> Result<NetworkConfig> {
    let polkadot_bin = get_polkadot_binary_path();
    let omni_node_bin = get_omni_node_binary_path();
    let chain_spec_cmd = get_chain_spec_command();

    log::info!("Polkadot binary: {polkadot_bin}");
    log::info!("Omni-node binary: {omni_node_bin}");
    log::info!("Chain spec command: {chain_spec_cmd}");

    let relay_args = vec!["-lruntime=info".into()];
    let relay_args2 = relay_args.clone();
    let para_args = vec!["--collator".into(), "-lruntime=info".into()];

    NetworkConfigBuilder::new()
        .with_relaychain(|rc| {
            rc.with_chain(RELAY_CHAIN)
                .with_default_command(polkadot_bin.as_str())
                .with_validator(|n| n.with_name("alice").with_args(relay_args))
                .with_validator(|n| n.with_name("bob").with_args(relay_args2))
        })
        .with_parachain(|pc| {
            pc.with_id(PARA_ID)
                .with_chain_spec_command(chain_spec_cmd.as_str())
                .cumulus_based(true)
                .with_collator(|c| {
                    c.with_name("collator-alice")
                        .validator(true)
                        .with_command(omni_node_bin.as_str())
                        .with_args(para_args)
                })
        })
        .with_global_settings(|gs| match std::env::var("ZOMBIENET_SDK_BASE_DIR") {
            Ok(val) => gs.with_base_dir(val),
            _ => gs,
        })
        .build()
        .map_err(|errs| {
            let message = errs
                .into_iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            anyhow!("Network config errors: {message}")
        })
}

/// Spawn a network from the given configuration using the native provider.
pub async fn spawn_network(config: NetworkConfig) -> Result<Network<LocalFileSystem>> {
    let network = config.spawn_native().await?;
    Ok(network)
}

/// Get the WebSocket URI for a collator node.
pub async fn wait_for_collator_ws(
    network: &Network<LocalFileSystem>,
    name: &str,
) -> Result<String> {
    let collator = network.get_node(name)?;

    log::info!("Waiting for {name} to be ready...");

    let ws_uri = collator.ws_uri().to_string();
    log::info!("{name} ready at: {ws_uri}");

    Ok(ws_uri)
}
