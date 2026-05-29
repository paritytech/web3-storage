//! End-to-end happy path
//! 1. register provider
//! 2. negotiate primary provider
//! 3. establish agreement, create bucket on-chain
//! 4. upload data
//! 5. download data
//!
//! Steps to run the test
//! 1. just start-paseo-chain
//! 2. cargo test -p storage-provider-node --test complete_workflow -- --no-capture

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use sp_runtime::AccountId32;
use storage_client::{
    AdminClient, ChunkingStrategy, ClientConfig, NegotiateRequest, ProviderClient,
    ProviderSettings, SignedTerms, StorageUserClient,
};
use storage_provider_node::{create_router, NonceCounter, ProviderState, Storage};
use subxt_signer::sr25519::{dev as dev_signer, Keypair};
use tokio::net::TcpListener;

const CHAIN_WS: &str = "ws://127.0.0.1:2222";
/// 1000 tokens × 12 decimals — covers the runtime's `MinProviderStake`.
const PROVIDER_STAKE: u128 = 1_000 * 1_000_000_000_000u128;

/// Bundles a dev-account's seed, dev-signer name, keypair, and derived
/// account id so the rest of the test pulls everything from one place.
struct DevIdentity {
    seed: &'static str,
    dev_name: &'static str,
    keypair: Keypair,
}

impl DevIdentity {
    fn provider() -> Self {
        Self {
            seed: "//Bob",
            dev_name: "bob",
            keypair: dev_signer::bob(),
        }
    }

    fn admin() -> Self {
        Self {
            seed: "//Alice",
            dev_name: "alice",
            keypair: dev_signer::alice(),
        }
    }

    fn account(&self) -> AccountId32 {
        AccountId32::from(self.keypair.public_key().0)
    }

    fn ss58(&self) -> String {
        self.account().to_string()
    }

    fn public_key_bytes(&self) -> Vec<u8> {
        self.keypair.public_key().0.to_vec()
    }
}

fn chain_config() -> ClientConfig {
    ClientConfig {
        chain_ws_url: CHAIN_WS.to_string(),
        provider_urls: vec![],
        timeout_secs: 30,
        enable_retries: false,
    }
}

/// In-process provider node signed with `id`'s seed.
async fn start_provider(id: &DevIdentity) -> (String, SocketAddr) {
    let storage = Arc::new(Storage::new());
    let mut state = ProviderState::with_seed(storage, id.seed).expect("provider keypair");
    state.nonce_counter = Some(Arc::new({
        let c = NonceCounter::in_memory(0);
        c.bootstrap_from_hwm(0);
        c
    }));
    let state = Arc::new(state);
    let app = create_router(state);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    (format!("http://{addr}"), addr)
}

/// Register `id` on-chain with the same sr25519 key its in-process provider
/// node uses for signing. Idempotent.
async fn ensure_provider_registered(id: &DevIdentity) -> Result<(), Box<dyn std::error::Error>> {
    let account = id.account();
    let mut provider = ProviderClient::new(chain_config(), id.ss58())?;
    provider.connect().await?;
    provider.set_dev_signer(id.dev_name)?;

    if provider.get_provider_info(&account).await?.is_none() {
        provider
            .register(
                "/ip4/127.0.0.1/tcp/3333".to_string(),
                id.public_key_bytes(),
                PROVIDER_STAKE,
            )
            .await?;

        provider
            .update_settings(ProviderSettings {
                price_per_byte: 0,
                min_duration: 1,
                max_duration: 1_000_000,
                accepting_primary: true,
                replica_sync_price: None,
                accepting_extensions: true,
                max_capacity: 0,
            })
            .await?;
    }
    Ok(())
}

#[tokio::test]
async fn complete_workflow_e2e() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let provider_id = DevIdentity::provider();
    let admin_id = DevIdentity::admin();

    tracing::info!("Ensuring provider registered on chain ...");
    ensure_provider_registered(&provider_id).await?;
    tracing::info!("Provider is already registered!");

    tracing::info!("Starting up provider");
    let (provider_url, _addr) = start_provider(&provider_id).await;

    tracing::info!("Admin submit `NegotiateRequest` and get provider's signature back.");
    // 1. Negotiate terms via the in-process provider node.
    let signed: SignedTerms = ProviderClient::negotiate_terms(
        &provider_url,
        &NegotiateRequest {
            owner: admin_id.account(),
            max_bytes: 1_000_000,
            duration: 100,
            price_per_byte: 0,
            replica_params: None,
        },
    )
    .await?;
    assert!(signed.terms.nonce > 0, "provider should allocate nonce");

    tracing::info!(
        "Admin establish storage agreement with primary provider and create on chain bucket."
    );
    // 2. Redeem on-chain.
    let mut admin = AdminClient::new(chain_config(), admin_id.ss58())?;
    admin.connect().await?;
    admin.set_dev_signer(admin_id.dev_name)?;
    let bucket_id = admin
        .establish_storage_agreement(provider_id.ss58(), signed.terms, signed.signature)
        .await?;
    tracing::info!("Bucket #{bucket_id} is created");

    tracing::info!("User upload & download the data, verify data integrity.");
    // 3. Upload + download against the same in-process provider node.
    let user_config = ClientConfig {
        chain_ws_url: CHAIN_WS.to_string(),
        provider_urls: vec![provider_url.clone()],
        timeout_secs: 30,
        enable_retries: false,
    };
    let user = StorageUserClient::new(user_config)?;
    let data = b"hello e2e".to_vec();
    let data_root = user
        .upload(bucket_id, &data, ChunkingStrategy::default())
        .await?;
    let downloaded = user.download(&data_root, 0, data.len() as u64).await?;
    assert_eq!(downloaded, data, "downloaded bytes must match upload");
    Ok(())
}
