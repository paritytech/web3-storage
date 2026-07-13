// SPDX-License-Identifier: GPL-3.0-only

//! Subxt-based production chain client shared by all coordinators.
//!
//! A single [`SubxtChainClient`] holds one [`subxt::OnlineClient`] connection
//! and one signing key, and implements every coordinator's chain-client trait
//! (`CheckpointChainClient`, `ReplicaSyncChainClient`,
//! `ChallengeChainClient`). Coordinators still depend on the narrow trait they
//! need, so per-trait mocks keep working; the production wiring just hands each
//! one a clone of the same client (a cheap `OnlineClient`/`Keypair` clone that
//! shares the underlying WebSocket connection).

use crate::chain_connection::{self, ChainWatch};
use crate::challenge_responder::{
    decode_challenge_for_provider, ChallengeChainClient, DetectedChallenge,
};
use crate::checkpoint_coordinator::{CheckpointChainClient, CheckpointDuty};
use crate::replica_sync_coordinator::{
    BucketSnapshot, ReplicaAgreementInfo, ReplicaSyncChainClient,
};
use crate::Error;
use sp_core::crypto::Ss58Codec;
use sp_core::H256;
use std::time::Duration;
use storage_primitives::BucketId;
use subxt::dynamic::Value;
use subxt::ext::scale_value::value;

/// Pallet dispatch errors that mean "this action already happened" — a retry
/// after a dropped transaction watch may race an earlier attempt that landed,
/// and these rejections prove the duty is done rather than failed.
const ALREADY_DONE_ERRORS: [&str; 3] = [
    "ChallengeNotFound", // respond_to_challenge: challenge was taken on defense
    "CheckpointAlreadySubmitted", // provider_checkpoint: window already covered
    "SyncTooFrequent",   // confirm_replica_sync: a sync already confirmed
];

/// Production implementation that talks to the chain via subxt.
///
/// Holds a receiver of the connection watch channel (owned by the chain-state
/// coordinator) plus this provider's signing key. Cloning is cheap, so every
/// background coordinator gets its own copy; each operation borrows the
/// current connection, which survives follower reconnects transparently.
#[derive(Clone)]
pub struct SubxtChainClient {
    chain_rx: ChainWatch,
    signer: subxt_signer::sr25519::Keypair,
}

impl SubxtChainClient {
    /// Create the signing chain client from the connection watch channel and
    /// the provider's seed URI (e.g. `//Alice` or a mnemonic), which
    /// reproduces the provider's registered account — the identity every
    /// on-chain action must be signed by.
    pub fn new(chain_rx: ChainWatch, seed: &str) -> Result<Self, Error> {
        let uri: subxt_signer::SecretUri = seed
            .parse()
            .map_err(|e| Error::Internal(format!("Invalid seed URI: {e}")))?;
        let signer = subxt_signer::sr25519::Keypair::from_uri(&uri)
            .map_err(|e| Error::Internal(format!("Failed to create signer: {e}")))?;

        tracing::info!(
            "Chain client signing as {}",
            sp_core::crypto::AccountId32::from(signer.public_key().0).to_ss58check()
        );

        Ok(Self { chain_rx, signer })
    }

    /// The current live connection, or an error while the chain has never
    /// been reached yet.
    fn api(&self) -> Result<subxt::OnlineClient<subxt::PolkadotConfig>, Error> {
        chain_connection::current_api(&self.chain_rx)
    }

    /// Get the current (latest) block number.
    ///
    /// Backs `get_current_block` on both the checkpoint and replica-sync traits.
    async fn current_block(&self) -> Result<u64, Error> {
        let at = self
            .api()?
            .at_current_block()
            .await
            .map_err(|e| Error::Internal(format!("Failed to get latest block: {e}")))?;
        Ok(at.block_number())
    }

    /// Sign, submit, and wait for finalized success, retrying once on
    /// transport-level failures (dropped socket, backend resubscription
    /// killing the transaction watch).
    ///
    /// On the retry, a dispatch error from [`ALREADY_DONE_ERRORS`] is treated
    /// as success: it means the first submission actually landed and the duty
    /// is complete. Matching on the formatted error is pragmatic — the names
    /// come from pallet metadata and are stable.
    async fn submit_and_finalize(
        &self,
        tx: &subxt::tx::DynamicPayload<Vec<Value>>,
        what: &str,
    ) -> Result<(), Error> {
        const RETRY_DELAY: Duration = Duration::from_secs(6);

        for attempt in 0..2 {
            let retrying = attempt > 0;
            let submitted = async {
                self.api()?
                    .at_current_block()
                    .await
                    .map_err(|e| Error::Internal(format!("Failed to get current block: {e}")))?
                    .transactions()
                    .sign_and_submit_then_watch_default(tx, &self.signer)
                    .await
                    .map_err(|e| Error::Internal(format!("Failed to submit tx: {e}")))
            }
            .await;

            let progress = match submitted {
                Ok(progress) => progress,
                Err(e) if !retrying => {
                    tracing::warn!("{what}: submission failed ({e}); retrying once");
                    tokio::time::sleep(RETRY_DELAY).await;
                    continue;
                }
                Err(e) => return Err(e),
            };

            return match progress.wait_for_finalized_success().await {
                Ok(_) => Ok(()),
                Err(e) => {
                    let msg = e.to_string();
                    if retrying && ALREADY_DONE_ERRORS.iter().any(|name| msg.contains(name)) {
                        tracing::info!(
                            "{what}: duplicate rejected on retry ({msg}); first attempt landed"
                        );
                        Ok(())
                    } else if !retrying && !msg.contains("Module") {
                        // Non-dispatch failure (e.g. the watch subscription
                        // died): the tx may or may not have landed — resubmit
                        // and let the duplicate classification above decide.
                        tracing::warn!("{what}: tx watch failed ({msg}); retrying once");
                        tokio::time::sleep(RETRY_DELAY).await;
                        continue;
                    } else {
                        Err(Error::Internal(format!("Transaction failed: {msg}")))
                    }
                }
            };
        }
        unreachable!("loop either returns or continues at most once")
    }

    /// Convert a multiaddr string to an HTTP endpoint.
    fn multiaddr_to_http_endpoint(multiaddr: &str) -> String {
        let parts: Vec<&str> = multiaddr.split('/').filter(|s| !s.is_empty()).collect();

        let mut host = "127.0.0.1".to_string();
        let mut port = "3333".to_string();

        let mut i = 0;
        while i < parts.len() {
            match parts[i] {
                "ip4" | "ip6" => {
                    if i + 1 < parts.len() {
                        host = parts[i + 1].to_string();
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                "dns4" | "dns6" | "dns" => {
                    if i + 1 < parts.len() {
                        host = parts[i + 1].to_string();
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                "tcp" => {
                    if i + 1 < parts.len() {
                        port = parts[i + 1].to_string();
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                _ => {
                    i += 1;
                }
            }
        }

        format!("http://{host}:{port}")
    }

    /// Convert a bind address (e.g. "0.0.0.0:3333") to a multiaddr string
    /// (e.g. "/ip4/127.0.0.1/tcp/3333").
    fn bind_addr_to_multiaddr(bind_addr: &str) -> String {
        let parts: Vec<&str> = bind_addr.split(':').collect();
        let (host, port) = if parts.len() == 2 {
            (parts[0], parts[1])
        } else {
            ("127.0.0.1", "3333")
        };
        // 0.0.0.0 isn't useful as a client-facing address
        let host = if host == "0.0.0.0" { "127.0.0.1" } else { host };
        format!("/ip4/{host}/tcp/{port}")
    }

    /// Extract the raw bytes of a decoded byte-sequence storage field
    /// (e.g. a `Vec<u8>` / `BoundedVec<u8>`).
    ///
    /// subxt's dynamic decoder represents such a field either as a flat
    /// sequence of byte primitives, or — for `BoundedVec` and similar newtype
    /// wrappers — as a single-element composite whose inner value holds the
    /// real sequence. We handle both, recursing through the wrapper layer. This
    /// mirrors the decoder behind the typed `storage_client` read path, so the
    /// shared connection here decodes a multiaddr exactly as that path would.
    fn extract_byte_vec<T>(val: &subxt::ext::scale_value::Value<T>) -> Vec<u8> {
        use subxt::ext::scale_value::{Composite, Primitive, ValueDef};
        match &val.value {
            ValueDef::Composite(Composite::Unnamed(items)) => {
                // Direct sequence of byte primitives.
                let bytes: Vec<u8> = items
                    .iter()
                    .filter_map(|item| match &item.value {
                        ValueDef::Primitive(Primitive::U128(n)) => Some(*n as u8),
                        _ => None,
                    })
                    .collect();
                if !items.is_empty() && bytes.len() == items.len() {
                    return bytes;
                }
                // BoundedVec wrapper: a single inner field holds the sequence.
                if items.len() == 1 {
                    return Self::extract_byte_vec(&items[0]);
                }
                Vec::new()
            }
            // Some subxt versions encode a single byte as a bare primitive.
            ValueDef::Primitive(Primitive::U128(n)) => vec![*n as u8],
            _ => Vec::new(),
        }
    }

    /// Ensure the provider's on-chain multiaddr matches the address it
    /// advertises.
    ///
    /// The advertised value is `public_multiaddr` when set (hosted deployments
    /// behind a reverse proxy), otherwise one derived from `bind_addr` (local
    /// dev). If the provider is registered and its recorded multiaddr differs,
    /// submit an `update_provider_multiaddr` transaction. Reuses this client's
    /// existing connection and signer rather than opening a new one.
    pub async fn sync_multiaddr(
        &self,
        provider_id: &str,
        bind_addr: &str,
        public_multiaddr: Option<&str>,
    ) {
        use subxt::dynamic::At;

        let expected_multiaddr = match public_multiaddr {
            Some(addr) => addr.to_string(),
            None => Self::bind_addr_to_multiaddr(bind_addr),
        };

        // Read current on-chain provider info
        let our_account: sp_core::crypto::AccountId32 =
            match sp_core::crypto::Ss58Codec::from_ss58check(provider_id) {
                Ok(a) => a,
                Err(_) => {
                    tracing::warn!("Invalid provider SS58 address, skipping multiaddr sync");
                    return;
                }
            };
        let our_bytes: [u8; 32] = our_account.into();

        let storage_query =
            subxt::dynamic::storage::<(Value,), Value>("StorageProvider", "Providers");

        let api = match self.api() {
            Ok(api) => api,
            Err(e) => {
                tracing::warn!("Failed to query storage for multiaddr sync: {}", e);
                return;
            }
        };
        let at = match api.at_current_block().await {
            Ok(at) => at,
            Err(e) => {
                tracing::warn!("Failed to query storage for multiaddr sync: {}", e);
                return;
            }
        };
        let result = at
            .storage()
            .try_fetch(storage_query, (Value::from_bytes(our_bytes),))
            .await;

        let provider_value = match result {
            Ok(Some(v)) => v,
            Ok(None) => {
                tracing::info!("Provider not registered on chain yet, skipping multiaddr sync");
                return;
            }
            Err(e) => {
                tracing::warn!("Failed to fetch provider info: {}", e);
                return;
            }
        };

        // Extract the current multiaddr from the encoded provider storage entry.
        let current = {
            let decoded = match provider_value.decode() {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!("Could not decode provider value: {}, skipping sync", e);
                    return;
                }
            };

            let multiaddr_val = match decoded.at("multiaddr") {
                Some(v) => v,
                None => {
                    tracing::warn!("No multiaddr field in provider info, skipping sync");
                    return;
                }
            };

            let bytes = Self::extract_byte_vec(multiaddr_val);
            if bytes.is_empty() {
                // Couldn't decode the stored multiaddr. Skip rather than treat
                // it as a mismatch — otherwise we'd submit a needless
                // update_provider_multiaddr transaction on every startup.
                tracing::warn!(
                    "Could not decode on-chain multiaddr (value: {:?}); skipping sync",
                    multiaddr_val
                );
                return;
            }
            String::from_utf8_lossy(&bytes).to_string()
        };

        if current == expected_multiaddr {
            tracing::info!(
                "On-chain multiaddr matches advertised address: {}",
                expected_multiaddr
            );
            return;
        }

        tracing::info!(
            "On-chain multiaddr mismatch: chain=\"{}\" actual=\"{}\", updating...",
            current,
            expected_multiaddr
        );

        let multiaddr_bytes = expected_multiaddr.as_bytes().to_vec();
        let tx = subxt::dynamic::tx(
            "StorageProvider",
            "update_provider_multiaddr",
            vec![Value::from_bytes(multiaddr_bytes)],
        );

        match self
            .submit_and_finalize(&tx, "update_provider_multiaddr")
            .await
        {
            Ok(()) => tracing::info!("Multiaddr updated on-chain to: {}", expected_multiaddr),
            Err(e) => tracing::error!("Multiaddr update tx failed: {}", e),
        }
    }

    /// Decode a storage agreement from raw SCALE-encoded bytes.
    fn decode_storage_agreement_bytes(
        bucket_id: BucketId,
        bytes: &[u8],
    ) -> Result<ReplicaAgreementInfo, Error> {
        // StorageAgreement layout:
        // - owner: AccountId (32 bytes)
        // - max_bytes: u64 (8 bytes)
        // - payment_locked: Balance (16 bytes)
        // - price_per_byte: Balance (16 bytes)
        // - expires_at: BlockNumber (4 bytes)
        // - extensions_blocked: bool (1 byte)
        // - role: ProviderRole (variable, enum)
        // - started_at: BlockNumber (4 bytes)

        let min_size = 32 + 8 + 16 + 16 + 4 + 1; // up to role enum
        if bytes.len() < min_size {
            return Err(Error::Internal("Agreement data too short".to_string()));
        }

        let role_start = 32 + 8 + 16 + 16 + 4 + 1; // Skip to role enum
        let role_variant = bytes.get(role_start).copied().unwrap_or(0);

        // Role enum: 0 = Primary, 1 = Replica
        if role_variant != 1 {
            return Err(Error::Internal("Not a replica agreement".to_string()));
        }

        // Parse Replica fields: sync_balance, sync_price, min_sync_interval, last_sync
        let replica_start = role_start + 1;
        let remaining = &bytes[replica_start..];

        if remaining.len() < 16 + 16 + 4 {
            return Err(Error::Internal("Replica data too short".to_string()));
        }

        let sync_balance = u128::from_le_bytes(
            remaining[0..16]
                .try_into()
                .map_err(|_| Error::Internal("Failed to parse sync_balance".to_string()))?,
        );

        let sync_price = u128::from_le_bytes(
            remaining[16..32]
                .try_into()
                .map_err(|_| Error::Internal("Failed to parse sync_price".to_string()))?,
        );

        let min_sync_interval = u32::from_le_bytes(
            remaining[32..36]
                .try_into()
                .map_err(|_| Error::Internal("Failed to parse min_sync_interval".to_string()))?,
        ) as u64;

        let last_sync_option = remaining.get(36).copied().unwrap_or(0);
        let last_sync = if last_sync_option == 1 && remaining.len() >= 36 + 1 + 32 + 4 {
            let root_bytes: [u8; 32] = remaining[37..69]
                .try_into()
                .map_err(|_| Error::Internal("Failed to parse last_sync root".to_string()))?;
            let block = u32::from_le_bytes(
                remaining[69..73]
                    .try_into()
                    .map_err(|_| Error::Internal("Failed to parse last_sync block".to_string()))?,
            ) as u64;
            Some((H256::from(root_bytes), block))
        } else {
            None
        };

        Ok(ReplicaAgreementInfo {
            bucket_id,
            sync_balance,
            sync_price,
            min_sync_interval,
            last_sync,
        })
    }

    /// Parse a BucketSnapshot value from scale_value.
    fn parse_bucket_snapshot_value<T>(value: &subxt::ext::scale_value::Value<T>) -> BucketSnapshot {
        use subxt::ext::scale_value::{At, Composite, Primitive, ValueDef};

        let mmr_root = if let Some(field0) = value.at(0) {
            if let ValueDef::Composite(Composite::Unnamed(bytes_vec)) = &field0.value {
                let bytes: Vec<u8> = bytes_vec
                    .iter()
                    .filter_map(|v| {
                        if let ValueDef::Primitive(Primitive::U128(n)) = &v.value {
                            Some(*n as u8)
                        } else {
                            None
                        }
                    })
                    .collect();
                if bytes.len() == 32 {
                    H256::from_slice(&bytes)
                } else {
                    H256::zero()
                }
            } else {
                H256::zero()
            }
        } else {
            H256::zero()
        };

        let leaf_count = if let Some(field2) = value.at(2) {
            if let ValueDef::Primitive(Primitive::U128(n)) = &field2.value {
                *n as u64
            } else {
                0
            }
        } else {
            0
        };

        BucketSnapshot {
            mmr_root,
            leaf_count,
        }
    }
}

#[async_trait::async_trait]
impl CheckpointChainClient for SubxtChainClient {
    async fn get_current_block(&self) -> Result<u64, Error> {
        self.current_block().await
    }

    async fn fetch_checkpoint_config(
        &self,
        bucket_id: BucketId,
    ) -> Result<Option<(u32, u32)>, Error> {
        use subxt::dynamic::At;

        let config_query =
            subxt::dynamic::storage::<(Value,), Value>("StorageProvider", "CheckpointConfigs");
        let at = self
            .api()?
            .at_current_block()
            .await
            .map_err(|e| Error::Internal(format!("Failed to get storage: {e}")))?;

        match at
            .storage()
            .try_fetch(config_query, (Value::u128(bucket_id as u128),))
            .await
            .map_err(|e| Error::Internal(format!("Failed to fetch config: {e}")))?
        {
            Some(val) => {
                let decoded = val
                    .decode()
                    .map_err(|e| Error::Internal(format!("Failed to decode config: {e}")))?;
                let interval = decoded
                    .at("interval")
                    .and_then(|v| v.as_u128())
                    .unwrap_or(100) as u32;
                let grace_period = decoded
                    .at("grace_period")
                    .and_then(|v| v.as_u128())
                    .unwrap_or(20) as u32;
                Ok(Some((interval, grace_period)))
            }
            None => Ok(None),
        }
    }

    async fn submit_checkpoint(
        &self,
        duty: &CheckpointDuty,
        signatures: Vec<(String, String)>,
    ) -> Result<H256, Error> {
        let bucket_id = duty.bucket_id;
        let mmr_root = duty.mmr_root;
        let start_seq = duty.start_seq;
        let leaf_count = duty.leaf_count;
        let window = duty.window;

        // Build signature tuples for the extrinsic
        let mut sig_values = Vec::with_capacity(signatures.len());
        for (account, sig) in &signatures {
            let account_id: sp_core::crypto::AccountId32 =
                sp_core::crypto::Ss58Codec::from_ss58check(account).map_err(|e| {
                    Error::Internal(format!("Invalid SS58 account '{account}': {e:?}"))
                })?;
            let account_bytes: [u8; 32] = account_id.into();

            let sig_bytes = hex::decode(sig.trim_start_matches("0x"))
                .map_err(|e| Error::Internal(format!("Invalid signature hex: {e}")))?;

            sig_values.push(value!((
                Value::from_bytes(account_bytes),
                Sr25519(Value::from_bytes(sig_bytes))
            )));
        }

        let tx = subxt::dynamic::tx(
            "StorageProvider",
            "provider_checkpoint",
            vec![
                Value::u128(bucket_id as u128),
                Value::named_composite(vec![
                    ("mmr_root", Value::from_bytes(mmr_root.as_bytes())),
                    ("start_seq", Value::u128(start_seq as u128)),
                    ("leaf_count", Value::u128(leaf_count as u128)),
                ]),
                Value::u128(window as u128),
                Value::unnamed_composite(sig_values),
            ],
        );

        self.submit_and_finalize(&tx, "provider_checkpoint").await?;

        Ok(H256::zero())
    }
}

#[async_trait::async_trait]
impl ReplicaSyncChainClient for SubxtChainClient {
    async fn get_current_block(&self) -> Result<u64, Error> {
        self.current_block().await
    }

    async fn fetch_replica_agreements(
        &self,
        provider_account: &str,
        local_buckets: Vec<BucketId>,
    ) -> Result<Vec<ReplicaAgreementInfo>, Error> {
        let provider_account = provider_account.to_string();
        {
            let mut agreements = Vec::new();

            let account_bytes = hex::decode(provider_account.trim_start_matches("0x"))
                .map_err(|e| Error::Internal(format!("Invalid account hex: {e}")))?;

            let api = self.api()?;

            // Query local buckets for agreements
            for bucket_id in &local_buckets {
                let storage_address = subxt::dynamic::storage::<(Value, Value), Value>(
                    "StorageProvider",
                    "StorageAgreements",
                );

                let at = api
                    .at_current_block()
                    .await
                    .map_err(|e| Error::Internal(format!("Failed to get storage: {e}")))?;

                if let Ok(Some(value)) = at
                    .storage()
                    .try_fetch(
                        storage_address,
                        (
                            Value::u128(*bucket_id as u128),
                            Value::from_bytes(&account_bytes),
                        ),
                    )
                    .await
                {
                    let encoded = value.bytes();
                    if let Ok(agreement) = Self::decode_storage_agreement_bytes(*bucket_id, encoded)
                    {
                        agreements.push(agreement);
                    }
                }
            }

            // Also iterate chain storage for agreements we might not have locally
            let storage_address = subxt::dynamic::storage::<(Value, Value), Value>(
                "StorageProvider",
                "StorageAgreements",
            );

            if let Ok(at) = api.at_current_block().await {
                if let Ok(mut iter) = at.storage().iter(storage_address, ()).await {
                    while let Some(result) = iter.next().await {
                        let kv = match result {
                            Ok(kv) => kv,
                            Err(e) => {
                                tracing::debug!("Error iterating storage: {e}");
                                continue;
                            }
                        };

                        let key_bytes = kv.key_bytes();
                        if key_bytes.len() < 32 + 16 + 8 + 16 + 32 {
                            continue;
                        }

                        let bucket_id_start = 32 + 16;
                        let bucket_id_bytes = &key_bytes[bucket_id_start..bucket_id_start + 8];
                        let bucket_id =
                            u64::from_le_bytes(bucket_id_bytes.try_into().unwrap_or([0; 8]));

                        let provider_start = bucket_id_start + 8 + 16;
                        let provider_bytes = &key_bytes[provider_start..];

                        if provider_bytes.len() < 32 || provider_bytes[..32] != account_bytes[..32]
                        {
                            continue;
                        }

                        let encoded = kv.value().bytes();
                        if let Ok(agreement) =
                            Self::decode_storage_agreement_bytes(bucket_id, encoded)
                        {
                            if !agreements
                                .iter()
                                .any(|a| a.bucket_id == agreement.bucket_id)
                            {
                                agreements.push(agreement);
                            }
                        }
                    }
                }
            }

            Ok(agreements)
        }
    }

    async fn fetch_bucket_snapshot(&self, bucket_id: BucketId) -> Result<BucketSnapshot, Error> {
        use subxt::ext::scale_value::ValueDef;

        let storage_address =
            subxt::dynamic::storage::<(Value,), Value>("StorageProvider", "Buckets");

        let at = self
            .api()?
            .at_current_block()
            .await
            .map_err(|e| Error::Internal(format!("Failed to get storage: {e}")))?;

        match at
            .storage()
            .try_fetch(storage_address, (Value::u128(bucket_id as u128),))
            .await
        {
            Ok(Some(value)) => {
                use subxt::ext::scale_value::At;
                let decoded = value
                    .decode()
                    .map_err(|e| Error::Internal(format!("Failed to decode bucket: {e}")))?;

                if let Some(snapshot_opt) = decoded.at(4) {
                    if let ValueDef::Variant(variant) = &snapshot_opt.value {
                        if variant.name == "Some" {
                            if let Some(snapshot_val) = variant.values.values().next() {
                                return Ok(Self::parse_bucket_snapshot_value(snapshot_val));
                            }
                        }
                    }
                }

                Ok(BucketSnapshot {
                    mmr_root: H256::zero(),
                    leaf_count: 0,
                })
            }
            _ => Ok(BucketSnapshot {
                mmr_root: H256::zero(),
                leaf_count: 0,
            }),
        }
    }

    async fn fetch_primary_endpoints(&self, bucket_id: BucketId) -> Result<Vec<String>, Error> {
        use subxt::ext::scale_value::{At, Composite, Primitive, ValueDef};

        let storage_address =
            subxt::dynamic::storage::<(Value,), Value>("StorageProvider", "Buckets");

        let at = self
            .api()?
            .at_current_block()
            .await
            .map_err(|e| Error::Internal(format!("Failed to get storage: {e}")))?;

        let bucket_value = match at
            .storage()
            .try_fetch(storage_address, (Value::u128(bucket_id as u128),))
            .await
        {
            Ok(Some(v)) => v,
            _ => return Ok(vec![]),
        };

        let decoded = bucket_value
            .decode()
            .map_err(|e| Error::Internal(format!("Failed to decode bucket: {e}")))?;

        let mut provider_bytes_list = Vec::new();

        // primary_providers is at index 3
        if let Some(field3) = decoded.at(3) {
            if let ValueDef::Composite(Composite::Unnamed(providers_vec)) = &field3.value {
                for provider_value in providers_vec {
                    if let ValueDef::Composite(Composite::Unnamed(account_bytes)) =
                        &provider_value.value
                    {
                        let bytes: Vec<u8> = account_bytes
                            .iter()
                            .filter_map(|v| {
                                if let ValueDef::Primitive(Primitive::U128(n)) = &v.value {
                                    Some(*n as u8)
                                } else {
                                    None
                                }
                            })
                            .collect();
                        if bytes.len() == 32 {
                            provider_bytes_list.push(bytes);
                        }
                    }
                }
            }
        }

        // Look up each provider's multiaddr
        let mut endpoints = Vec::new();
        for provider_bytes in provider_bytes_list {
            let provider_addr =
                subxt::dynamic::storage::<(Value,), Value>("StorageProvider", "Providers");

            let at = self
                .api()?
                .at_current_block()
                .await
                .map_err(|e| Error::Internal(format!("Failed to get storage: {e}")))?;

            if let Ok(Some(value)) = at
                .storage()
                .try_fetch(provider_addr, (Value::from_bytes(&provider_bytes),))
                .await
            {
                if let Ok(decoded) = value.decode() {
                    if let Some(field0) = decoded.at(0) {
                        let bytes = Self::extract_byte_vec(field0);
                        if !bytes.is_empty() {
                            let multiaddr_str = String::from_utf8_lossy(&bytes);
                            endpoints.push(Self::multiaddr_to_http_endpoint(&multiaddr_str));
                        }
                    }
                }
            }
        }

        Ok(endpoints)
    }

    async fn submit_sync_confirmation(
        &self,
        bucket_id: BucketId,
        target_mmr_root: H256,
    ) -> Result<(u8, u128), Error> {
        // Build roots array: position 0 = current root, rest = None
        let roots_value: Vec<Value> = (0..7)
            .map(|i| {
                if i == 0 {
                    value!(Some(Value::from_bytes(target_mmr_root.as_bytes())))
                } else {
                    value!(None())
                }
            })
            .collect();

        let signature = value!(Sr25519(Value::from_bytes([0u8; 64])));

        let tx = subxt::dynamic::tx(
            "StorageProvider",
            "confirm_replica_sync",
            vec![
                Value::u128(bucket_id as u128),
                Value::unnamed_composite(roots_value),
                signature,
            ],
        );

        tracing::info!(
            "Submitting confirm_replica_sync for bucket {} with root 0x{}",
            bucket_id,
            hex::encode(target_mmr_root.as_bytes())
        );

        self.submit_and_finalize(&tx, "confirm_replica_sync")
            .await?;

        tracing::info!(
            "confirm_replica_sync submitted successfully for bucket {}",
            bucket_id
        );

        Ok((0, 0)) // Position 0, payment extracted from events in production
    }
}

#[async_trait::async_trait]
impl ChallengeChainClient for SubxtChainClient {
    /// Poll for active challenges against this provider.
    ///
    /// Iterates the on-chain `StorageProvider::Challenges` map. It is a
    /// `StorageDoubleMap<BlockNumber deadline, u16 index, Challenge>`, so each
    /// key holds exactly one challenge. We scan all entries, decode each
    /// single `Challenge` from raw SCALE, and keep only the ones whose
    /// `provider` matches our account.
    ///
    /// Cost is bounded by `ChallengeTimeout` (storage entries past their
    /// deadline are reaped in `on_finalize`), so iteration is at worst the
    /// number of open challenges across all unexpired deadlines.
    async fn poll_challenges(&self) -> Result<Vec<DetectedChallenge>, Error> {
        // Our account's raw bytes, used to filter challenges targeting us.
        let our_bytes: [u8; 32] = self.signer.public_key().0;

        let storage_address =
            subxt::dynamic::storage::<(Value, Value), Value>("StorageProvider", "Challenges");
        let at = self
            .api()?
            .at_current_block()
            .await
            .map_err(|e| Error::Internal(format!("Failed to get storage: {e}")))?;

        let mut iter = at
            .storage()
            .iter(storage_address, ())
            .await
            .map_err(|e| Error::Internal(format!("Failed to iterate Challenges: {e}")))?;

        let mut detected = Vec::new();
        while let Some(result) = iter.next().await {
            let kv = match result {
                Ok(kv) => kv,
                Err(e) => {
                    tracing::debug!("Error iterating Challenges: {e}");
                    continue;
                }
            };

            // DoubleMap key layout:
            //   16 (pallet hash) + 16 (storage hash)
            //   + 16 (blake2_128 of deadline) + 4 (BlockNumber u32 LE)
            //   + 8 (twox64 of index)         + 2 (u16 index LE)
            //   = 62 bytes total.
            let key_bytes = kv.key_bytes();
            if key_bytes.len() < 62 {
                continue;
            }
            let deadline = u32::from_le_bytes(key_bytes[48..52].try_into().unwrap_or([0; 4]));
            let index = u16::from_le_bytes(key_bytes[60..62].try_into().unwrap_or([0; 2]));

            let encoded = kv.value().bytes();
            let challenge = match decode_challenge_for_provider(encoded, &our_bytes) {
                Ok(Some(c)) => c,
                Ok(None) => continue,
                Err(e) => {
                    tracing::warn!("Failed to decode challenge at {deadline}/{index}: {e}");
                    continue;
                }
            };

            detected.push(DetectedChallenge {
                bucket_id: challenge.bucket_id,
                deadline,
                index,
                mmr_root: challenge.mmr_root,
                start_seq: challenge.start_seq,
                leaf_index: challenge.leaf_index,
                chunk_index: challenge.chunk_index,
                challenger: sp_core::crypto::AccountId32::from(challenge.challenger).to_ss58check(),
            });
        }
        Ok(detected)
    }

    /// Point-read a single challenge at `(deadline, index)`.
    ///
    /// Backs the event-driven path: a `ChallengeCreated` event carries the
    /// challenge id but not the proof parameters, so the responder fetches
    /// the full `Challenge` value here. Returns `None` when the entry is
    /// missing (already responded / reaped) or targets another provider.
    async fn fetch_challenge(
        &self,
        deadline: u32,
        index: u16,
    ) -> Result<Option<DetectedChallenge>, Error> {
        let our_bytes: [u8; 32] = self.signer.public_key().0;

        let storage_address =
            subxt::dynamic::storage::<(Value, Value), Value>("StorageProvider", "Challenges");
        let at = self
            .api()?
            .at_current_block()
            .await
            .map_err(|e| Error::Internal(format!("Failed to get storage: {e}")))?;

        let Some(value) = at
            .storage()
            .try_fetch(
                storage_address,
                (Value::u128(deadline as u128), Value::u128(index as u128)),
            )
            .await
            .map_err(|e| Error::Internal(format!("Failed to fetch challenge: {e}")))?
        else {
            return Ok(None);
        };

        let challenge = match decode_challenge_for_provider(value.bytes(), &our_bytes) {
            Ok(Some(c)) => c,
            Ok(None) => return Ok(None),
            Err(e) => {
                return Err(Error::Internal(format!(
                    "Failed to decode challenge at {deadline}/{index}: {e}"
                )))
            }
        };

        Ok(Some(DetectedChallenge {
            bucket_id: challenge.bucket_id,
            deadline,
            index,
            mmr_root: challenge.mmr_root,
            start_seq: challenge.start_seq,
            leaf_index: challenge.leaf_index,
            chunk_index: challenge.chunk_index,
            challenger: sp_core::crypto::AccountId32::from(challenge.challenger).to_ss58check(),
        }))
    }

    async fn submit_response(
        &self,
        challenge_id: (u32, u16),
        chunk_data: Vec<u8>,
        mmr_proof: storage_primitives::MmrProof,
        chunk_proof: storage_primitives::MerkleProof,
    ) -> Result<H256, Error> {
        let challenge_id_val = value!({
            deadline: challenge_id.0 as u128,
            index: challenge_id.1 as u128
        });

        // Dynamic arrays built from iterators, then embedded in the macro
        let peaks = Value::unnamed_composite(
            mmr_proof
                .peaks
                .iter()
                .map(|p| Value::from_bytes(p.as_bytes()))
                .collect::<Vec<_>>(),
        );
        let mmr_siblings = Value::unnamed_composite(
            mmr_proof
                .leaf_proof
                .siblings
                .iter()
                .map(|s| Value::from_bytes(s.as_bytes()))
                .collect::<Vec<_>>(),
        );
        let mmr_path = Value::unnamed_composite(
            mmr_proof
                .leaf_proof
                .path
                .iter()
                .map(|b| Value::bool(*b))
                .collect::<Vec<_>>(),
        );

        let mmr_proof_val = value!({
            peaks: peaks,
            leaf: {
                data_root: Value::from_bytes(mmr_proof.leaf.data_root.as_bytes()),
                data_size: mmr_proof.leaf.data_size as u128,
                total_size: mmr_proof.leaf.total_size as u128
            },
            leaf_proof: {
                siblings: mmr_siblings,
                path: mmr_path
            }
        });

        let chunk_siblings = Value::unnamed_composite(
            chunk_proof
                .siblings
                .iter()
                .map(|s| Value::from_bytes(s.as_bytes()))
                .collect::<Vec<_>>(),
        );
        let chunk_path = Value::unnamed_composite(
            chunk_proof
                .path
                .iter()
                .map(|b| Value::bool(*b))
                .collect::<Vec<_>>(),
        );

        let chunk_proof_val = value!({
            siblings: chunk_siblings,
            path: chunk_path
        });

        let response_val = value!(Proof {
            chunk_data: Value::from_bytes(&chunk_data),
            mmr_proof: mmr_proof_val,
            chunk_proof: chunk_proof_val
        });

        let tx = subxt::dynamic::tx(
            "StorageProvider",
            "respond_to_challenge",
            vec![challenge_id_val, response_val],
        );

        self.submit_and_finalize(&tx, "respond_to_challenge")
            .await?;

        Ok(H256::zero())
    }
}
