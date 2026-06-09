//! Subxt-based production chain client for the replica sync coordinator.

use crate::replica_sync_coordinator::{
    BucketSnapshot, ReplicaAgreementInfo, ReplicaSyncChainClient,
};
use crate::Error;
use sp_core::H256;
use storage_primitives::BucketId;

/// Production implementation that talks to the chain via subxt.
pub struct SubxtReplicaSyncChainClient {
    api: subxt::OnlineClient<subxt::PolkadotConfig>,
    signer: subxt_signer::sr25519::Keypair,
}

impl SubxtReplicaSyncChainClient {
    /// Connect to the chain and create a signer from the provider state's keypair.
    pub async fn connect(
        chain_ws_url: &str,
        keypair: &sp_core::sr25519::Pair,
    ) -> Result<Self, Error> {
        use sp_core::Pair;

        let api = subxt::OnlineClient::<subxt::PolkadotConfig>::from_url(chain_ws_url)
            .await
            .map_err(|e| Error::Internal(format!("Failed to connect to chain: {e}")))?;

        let raw = keypair.to_raw_vec();
        let secret_bytes: [u8; 32] = raw[..32]
            .try_into()
            .map_err(|_| Error::Internal("Invalid secret key length".to_string()))?;
        let signer = subxt_signer::sr25519::Keypair::from_secret_key(secret_bytes)
            .map_err(|e| Error::Internal(format!("Failed to create signer: {e}")))?;

        tracing::info!("Replica sync coordinator connected to {}", chain_ws_url);

        Ok(Self { api, signer })
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
impl ReplicaSyncChainClient for SubxtReplicaSyncChainClient {
    async fn get_current_block(&self) -> Result<u64, Error> {
        let block = self
            .api
            .blocks()
            .at_latest()
            .await
            .map_err(|e| Error::Internal(format!("Failed to get latest block: {e}")))?;
        Ok(block.number() as u64)
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

            // Query local buckets for agreements
            for bucket_id in &local_buckets {
                let storage_address = subxt::dynamic::storage(
                    "StorageProvider",
                    "StorageAgreements",
                    vec![
                        subxt::dynamic::Value::u128(*bucket_id as u128),
                        subxt::dynamic::Value::from_bytes(&account_bytes),
                    ],
                );

                let storage = self
                    .api
                    .storage()
                    .at_latest()
                    .await
                    .map_err(|e| Error::Internal(format!("Failed to get storage: {e}")))?;

                if let Ok(Some(value)) = storage.fetch(&storage_address).await {
                    let encoded = value.encoded();
                    if let Ok(agreement) = Self::decode_storage_agreement_bytes(*bucket_id, encoded)
                    {
                        agreements.push(agreement);
                    }
                }
            }

            // Also iterate chain storage for agreements we might not have locally
            let storage_address =
                subxt::dynamic::storage("StorageProvider", "StorageAgreements", ());

            if let Ok(storage) = self.api.storage().at_latest().await {
                if let Ok(mut iter) = storage.iter(storage_address).await {
                    while let Some(result) = iter.next().await {
                        let kv = match result {
                            Ok(kv) => kv,
                            Err(e) => {
                                tracing::debug!("Error iterating storage: {e}");
                                continue;
                            }
                        };

                        let key_bytes = kv.key_bytes;
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

                        let encoded = kv.value.encoded();
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

        let storage_address = subxt::dynamic::storage(
            "StorageProvider",
            "Buckets",
            vec![subxt::dynamic::Value::u128(bucket_id as u128)],
        );

        let storage = self
            .api
            .storage()
            .at_latest()
            .await
            .map_err(|e| Error::Internal(format!("Failed to get storage: {e}")))?;

        match storage.fetch(&storage_address).await {
            Ok(Some(value)) => {
                use subxt::ext::scale_value::At;
                let decoded = value
                    .to_value()
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

        let storage_address = subxt::dynamic::storage(
            "StorageProvider",
            "Buckets",
            vec![subxt::dynamic::Value::u128(bucket_id as u128)],
        );

        let storage = self
            .api
            .storage()
            .at_latest()
            .await
            .map_err(|e| Error::Internal(format!("Failed to get storage: {e}")))?;

        let bucket_value = match storage.fetch(&storage_address).await {
            Ok(Some(v)) => v,
            _ => return Ok(vec![]),
        };

        let decoded = bucket_value
            .to_value()
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
            let provider_addr = subxt::dynamic::storage(
                "StorageProvider",
                "Providers",
                vec![subxt::dynamic::Value::from_bytes(&provider_bytes)],
            );

            let storage = self
                .api
                .storage()
                .at_latest()
                .await
                .map_err(|e| Error::Internal(format!("Failed to get storage: {e}")))?;

            if let Ok(Some(value)) = storage.fetch(&provider_addr).await {
                if let Ok(decoded) = value.to_value() {
                    if let Some(field0) = decoded.at(0) {
                        if let ValueDef::Composite(Composite::Unnamed(multiaddr_bytes)) =
                            &field0.value
                        {
                            let bytes: Vec<u8> = multiaddr_bytes
                                .iter()
                                .filter_map(|v| {
                                    if let ValueDef::Primitive(Primitive::U128(n)) = &v.value {
                                        Some(*n as u8)
                                    } else {
                                        None
                                    }
                                })
                                .collect();
                            if !bytes.is_empty() {
                                let multiaddr_str = String::from_utf8_lossy(&bytes);
                                endpoints.push(Self::multiaddr_to_http_endpoint(&multiaddr_str));
                            }
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
        let roots_value: Vec<subxt::dynamic::Value> = (0..7)
            .map(|i| {
                if i == 0 {
                    subxt::dynamic::Value::unnamed_variant(
                        "Some",
                        vec![subxt::dynamic::Value::from_bytes(
                            target_mmr_root.as_bytes(),
                        )],
                    )
                } else {
                    subxt::dynamic::Value::unnamed_variant("None", vec![])
                }
            })
            .collect();

        let signature = subxt::dynamic::Value::unnamed_variant(
            "Sr25519",
            vec![subxt::dynamic::Value::from_bytes([0u8; 64])],
        );

        let tx = subxt::dynamic::tx(
            "StorageProvider",
            "confirm_replica_sync",
            vec![
                subxt::dynamic::Value::u128(bucket_id as u128),
                subxt::dynamic::Value::unnamed_composite(roots_value),
                signature,
            ],
        );

        tracing::info!(
            "Submitting confirm_replica_sync for bucket {} with root 0x{}",
            bucket_id,
            hex::encode(target_mmr_root.as_bytes())
        );

        let tx_progress = self
            .api
            .tx()
            .sign_and_submit_then_watch_default(&tx, &self.signer)
            .await
            .map_err(|e| Error::Internal(format!("Failed to submit tx: {e}")))?;

        let _events = tx_progress
            .wait_for_finalized_success()
            .await
            .map_err(|e| Error::Internal(format!("Transaction failed: {e}")))?;

        tracing::info!(
            "confirm_replica_sync submitted successfully for bucket {}",
            bucket_id
        );

        Ok((0, 0)) // Position 0, payment extracted from events in production
    }
}
