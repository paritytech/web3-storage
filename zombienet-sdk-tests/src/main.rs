use zombienet_sdk_tests::{config, network, provider::ProviderProcess};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let config = network::build_network_config()?;
    let network = network::spawn_network(config).await?;
    let chain_ws = network::wait_for_collator_ws(&network, "collator-alice").await?;

    // Start provider if --with-provider flag is passed
    let provider = if std::env::args().any(|a| a == "--with-provider") {
        log::info!("Starting storage provider...");
        Some(ProviderProcess::spawn(&chain_ws).await?)
    } else {
        None
    };

    let encoded_ws = chain_ws.replace(':', "%3A").replace('/', "%2F");

    println!();
    println!("=== Network Ready ===");
    println!();
    println!("  Parachain WS:  {chain_ws}");
    println!("  Polkadot.js:   https://polkadot.js.org/apps/?rpc={encoded_ws}");
    if provider.is_some() {
        println!(
            "  Provider HTTP: http://127.0.0.1:{}",
            config::PROVIDER_PORT
        );
        println!(
            "  Provider health: http://127.0.0.1:{}/health",
            config::PROVIDER_PORT
        );
    }
    println!();
    println!("Press Ctrl+C to stop.");
    println!();

    tokio::signal::ctrl_c().await?;
    println!("Shutting down...");

    // Drop provider first, then network goes out of scope
    drop(provider);
    drop(network);

    Ok(())
}
