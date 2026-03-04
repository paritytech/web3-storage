use zombienet_sdk_tests::{config, network, provider::ProviderProcess};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let network_config = network::build_network_config()?;
    let network = network::spawn_network(network_config).await?;
    let chain_ws = network::get_collator_ws(&network, "collator-alice")?;

    // Start provider if --with-provider flag is passed
    let provider = if std::env::args().any(|a| a == "--with-provider") {
        log::info!("Starting storage provider...");
        Some(ProviderProcess::spawn(&chain_ws, "//Alice").await?)
    } else {
        None
    };

    let chain_rpc_port = config::chain_rpc_port();
    let relay_rpc_port = config::relay_rpc_port();
    let provider_port = config::provider_port();

    log::info!("=== Network Ready ===");
    log::info!("Relay WS: ws://127.0.0.1:{relay_rpc_port}");
    log::info!("Parachain WS: ws://127.0.0.1:{chain_rpc_port}");
    log::info!(
        "Polkadot.js: https://polkadot.js.org/apps/?rpc=ws%3A%2F%2F127.0.0.1%3A{chain_rpc_port}"
    );
    if provider.is_some() {
        log::info!("Provider HTTP: http://127.0.0.1:{provider_port}");
        log::info!("Provider health: http://127.0.0.1:{provider_port}/health");
    }
    log::info!("export CHAIN_WS=ws://127.0.0.1:{chain_rpc_port}");
    log::info!("export PROVIDER_URL=http://127.0.0.1:{provider_port}");
    log::info!("Press Ctrl+C to stop.");

    tokio::signal::ctrl_c().await?;
    log::info!("Shutting down...");

    // Drop provider first, then network goes out of scope
    drop(provider);
    drop(network);

    Ok(())
}
