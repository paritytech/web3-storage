//! Shared test helpers for integration tests.

use std::sync::Arc;
use storage_client::{ClientConfig, StorageUserClient};
use storage_provider_node::{create_router, ProviderState, Storage};
use tokio::net::TcpListener;

/// Spawn an in-process provider node on a random port and return its URL.
pub async fn start_test_provider() -> String {
    let storage = Arc::new(Storage::new());
    let state = Arc::new(ProviderState::new(storage, "0xtest_provider".to_string()));
    let app = create_router(state);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // Give the OS a moment to hand off the socket.
    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

    format!("http://{addr}")
}

/// Spawn `n` independent in-process provider nodes.
#[allow(dead_code)]
pub async fn start_providers(n: usize) -> Vec<String> {
    let mut urls = Vec::with_capacity(n);
    for _ in 0..n {
        urls.push(start_test_provider().await);
    }
    urls
}

/// Build a `StorageUserClient` pointed at the given provider URL.
/// The chain WS URL is a placeholder — these integration tests never touch the chain.
pub fn make_client(provider_url: String) -> StorageUserClient {
    StorageUserClient::new(ClientConfig {
        chain_ws_url: "ws://127.0.0.1:19999".to_string(),
        provider_urls: vec![provider_url],
        timeout_secs: 10,
        enable_retries: false,
    })
    .expect("ClientConfig should be valid")
}
