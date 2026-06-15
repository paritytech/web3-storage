//! Finalized-block event subscriber example.
//!
//! Subscribes to finalized blocks and runs every block's events through a registry
//! of pluggable parsers. Each plugin parses only the events it understands and
//! receives them via its own callback.
//!
//! Prerequisites:
//!   1. Start the chain: just start-chain
//!
//! Then run a flow that emits events (e.g. `cargo run --example complete_workflow`)
//! in another terminal to see the events printed here.
//!
//! Usage: cargo run --example block_subscription [chain_ws]
//!
//! Arguments:
//!   chain_ws  - WebSocket URL for parachain  (default: ws://127.0.0.1:2222)

use std::env;
use storage_client::block_subscription::{BlockSubscriber, ParserPlugin};
use storage_client::event_subscription::{StorageEvent, StorageProviderEventParser};

const DEFAULT_CHAIN_WS: &str = "ws://127.0.0.1:2222";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let chain_ws = env::args()
        .nth(1)
        .unwrap_or_else(|| DEFAULT_CHAIN_WS.to_string());

    println!("Connecting to {chain_ws} ...");

    // Register a plugin built from `StorageProviderEventParser`. Two equivalent
    // forms are shown:
    let mut subscriber = BlockSubscriber::connect(&chain_ws).await?.register(
        ParserPlugin::<StorageEvent, StorageProviderEventParser, _>::new(
            "storage-provider",
            |events: Vec<StorageEvent>| {
                for ev in events {
                    println!("[storage-provider] {ev:?}");
                }
            },
        ),
    );

    subscriber.start().await?;
    println!(
        "Subscribed to finalized blocks with {} plugin(s). Press Ctrl-C to stop.",
        subscriber.plugin_count()
    );

    tokio::signal::ctrl_c().await?;
    println!("\nStopping ...");
    subscriber.stop().await;

    Ok(())
}