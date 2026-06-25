// SPDX-License-Identifier: GPL-3.0-only

//! Subxt-based production chain client shared by all coordinators.
//!
//! A single [`SubxtChainClient`] holds one [`SubstrateClient`] connection
//! and one signing key, and implements every coordinator's chain-client trait
//! (`CheckpointChainClient`, `ReplicaSyncChainClient`,
//! `ChallengeChainClient`). Coordinators still depend on the narrow trait they
//! need, so per-trait mocks keep working; the production wiring just hands each
//! one a clone of the same client (a cheap `SubstrateClient` clone that shares
//! the underlying WebSocket connection).

use crate::challenge_responder::{ChallengeChainClient, DetectedChallenge};
use crate::checkpoint_coordinator::{CheckpointChainClient, CheckpointDuty};
use crate::replica_sync_coordinator::{
    BucketSnapshot, ReplicaAgreementInfo, ReplicaSyncChainClient,
};
use crate::Error;
use sp_core::crypto::Ss58Codec;
use sp_core::H256;
use sp_runtime::AccountId32;
use storage_client::substrate::{extrinsics, storage, SubstrateClient};
use storage_primitives::BucketId;
use storage_subxt::storage_runtime::api::runtime_types as rt;
use storage_subxt::subxt_signer;

/// Production implementation that talks to the chain via typed storage_client bindings.
///
/// Cloning is cheap: `SubstrateClient` shares one connection behind an `Arc`, and
/// the `Keypair` clone is a key copy. This lets a single instance be shared
/// across every background coordinator.
#[derive(Clone)]
pub struct SubxtChainClient {
    client: SubstrateClient,
}

impl SubxtChainClient {
    /// Connect to the chain and create a signer from the seed URI.
    pub async fn connect(chain_ws_url: &str, seed: &str) -> Result<Self, Error> {
        let uri: subxt_signer::SecretUri = seed
            .parse()
            .map_err(|e| Error::Internal(format!("Invalid seed URI: {e}")))?;
        let keypair = subxt_signer::sr25519::Keypair::from_uri(&uri)
            .map_err(|e| Error::Internal(format!("Failed to create signer: {e}")))?;

        tracing::info!(
            "Chain client connected to {} as {}",
            chain_ws_url,
            sp_core::crypto::AccountId32::from(keypair.public_key().0).to_ss58check()
        );

        let client = SubstrateClient::connect(chain_ws_url)
            .await
            .map_err(|e| Error::Internal(format!("Failed to connect to chain: {e}")))?
            .with_signer(keypair);

        Ok(Self { client })
    }

    /// Get the current (latest) block number.
    async fn current_block(&self) -> Result<u64, Error> {
        let block = self
            .client
            .api()
            .blocks()
            .at_latest()
            .await
            .map_err(|e| Error::Internal(format!("Failed to get latest block: {e}")))?;
        Ok(block.number() as u64)
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
        let host = if host == "0.0.0.0" { "127.0.0.1" } else { host };
        format!("/ip4/{host}/tcp/{port}")
    }

    /// Ensure the provider's on-chain multiaddr matches the address it advertises.
    pub async fn sync_multiaddr(
        &self,
        provider_id: &str,
        bind_addr: &str,
        public_multiaddr: Option<&str>,
    ) {
        let expected_multiaddr = match public_multiaddr {
            Some(addr) => addr.to_string(),
            None => Self::bind_addr_to_multiaddr(bind_addr),
        };

        let account = match SubstrateClient::parse_account(provider_id) {
            Ok(a) => a,
            Err(_) => {
                tracing::warn!("Invalid provider SS58 address, skipping multiaddr sync");
                return;
            }
        };

        let storage_api = match self.client.api().storage().at_latest().await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("Failed to query storage for multiaddr sync: {}", e);
                return;
            }
        };

        let provider_info = match storage_api.fetch(&storage::provider_info(&account)).await {
            Ok(Some(info)) => info,
            Ok(None) => {
                tracing::info!("Provider not registered on chain yet, skipping multiaddr sync");
                return;
            }
            Err(e) => {
                tracing::warn!("Failed to fetch provider info: {}", e);
                return;
            }
        };

        let current = String::from_utf8_lossy(&provider_info.multiaddr.0).to_string();
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

        let tx = extrinsics::update_provider_multiaddr(expected_multiaddr.as_bytes().to_vec());
        let signer = match self.client.signer() {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("No signer available for multiaddr update: {}", e);
                return;
            }
        };
        match self
            .client
            .api()
            .tx()
            .sign_and_submit_then_watch_default(&tx, signer)
            .await
        {
            Ok(progress) => match progress.wait_for_finalized_success().await {
                Ok(_) => {
                    tracing::info!("Multiaddr updated on-chain to: {}", expected_multiaddr)
                }
                Err(e) => tracing::error!("Multiaddr update tx failed: {}", e),
            },
            Err(e) => {
                tracing::error!("Failed to submit multiaddr update: {}", e);
            }
        }
    }

    /// Extract `ReplicaAgreementInfo` from a typed `StorageAgreement` if it is a Replica.
    fn to_replica_info(
        bucket_id: BucketId,
        agreement: &rt::pallet_storage_provider::pallet::StorageAgreement,
    ) -> Option<ReplicaAgreementInfo> {
        match &agreement.role {
            rt::storage_primitives::ProviderRole::Replica {
                sync_balance,
                sync_price,
                min_sync_interval,
                last_sync,
            } => Some(ReplicaAgreementInfo {
                bucket_id,
                sync_balance: *sync_balance,
                sync_price: *sync_price,
                min_sync_interval: *min_sync_interval as u64,
                last_sync: last_sync.as_ref().map(|(h, b)| (*h, *b as u64)),
            }),
            rt::storage_primitives::ProviderRole::Primary => None,
        }
    }

    /// Extract bucket_id from a StorageAgreements double-map key if the second key matches
    /// `account_bytes`. Layout (Blake2_128Concat×2): 16+16 pallet/storage hashes,
    /// 16+8 key1, 16+32 key2.
    fn extract_bucket_if_provider(key_bytes: &[u8], account_bytes: &[u8]) -> Option<BucketId> {
        if key_bytes.len() < 16 + 16 + 16 + 8 + 16 + 32 {
            return None;
        }
        let bucket_id_start = 32 + 16;
        let bucket_id = u64::from_le_bytes(
            key_bytes[bucket_id_start..bucket_id_start + 8]
                .try_into()
                .ok()?,
        );
        let provider_start = bucket_id_start + 8 + 16;
        if key_bytes[provider_start..provider_start + 32] == account_bytes[..32] {
            Some(bucket_id)
        } else {
            None
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
        let storage_api = self
            .client
            .api()
            .storage()
            .at_latest()
            .await
            .map_err(|e| Error::Internal(format!("Failed to get storage: {e}")))?;

        match storage_api
            .fetch(&storage::checkpoint_config(bucket_id))
            .await
            .map_err(|e| Error::Internal(format!("Failed to fetch config: {e}")))?
        {
            Some(config) => Ok(Some((config.interval, config.grace_period))),
            None => Ok(None),
        }
    }

    async fn submit_checkpoint(
        &self,
        duty: &CheckpointDuty,
        signatures: Vec<(String, String)>,
    ) -> Result<H256, Error> {
        let mut sig_vec = Vec::with_capacity(signatures.len());
        for (account, sig) in &signatures {
            let account_id = SubstrateClient::parse_account(account)
                .map_err(|e| Error::Internal(format!("Invalid SS58 account '{account}': {e}")))?;
            let sig_bytes = hex::decode(sig.trim_start_matches("0x"))
                .map_err(|e| Error::Internal(format!("Invalid signature hex: {e}")))?;
            sig_vec.push((account_id, sig_bytes));
        }

        let tx = extrinsics::provider_checkpoint(
            duty.bucket_id,
            duty.mmr_root,
            duty.start_seq,
            duty.leaf_count,
            duty.window,
            sig_vec,
        );

        let signer = self
            .client
            .signer()
            .map_err(|e| Error::Internal(e.to_string()))?;
        let tx_progress = self
            .client
            .api()
            .tx()
            .sign_and_submit_then_watch_default(&tx, signer)
            .await
            .map_err(|e| Error::Internal(format!("Failed to submit tx: {e}")))?;

        tx_progress
            .wait_for_finalized_success()
            .await
            .map_err(|e| Error::Internal(format!("Transaction failed: {e}")))?;

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
        let account_bytes = hex::decode(provider_account.trim_start_matches("0x"))
            .map_err(|e| Error::Internal(format!("Invalid account hex: {e}")))?;
        if account_bytes.len() < 32 {
            return Err(Error::Internal("Account bytes too short".to_string()));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&account_bytes[..32]);
        let account = AccountId32::from(arr);

        let mut agreements = Vec::new();

        let storage_api = self
            .client
            .api()
            .storage()
            .at_latest()
            .await
            .map_err(|e| Error::Internal(format!("Failed to get storage: {e}")))?;

        // Specific queries for locally-known buckets
        for bucket_id in &local_buckets {
            if let Ok(Some(agreement)) = storage_api
                .fetch(&storage::agreement_info(*bucket_id, &account))
                .await
            {
                if let Some(info) = Self::to_replica_info(*bucket_id, &agreement) {
                    agreements.push(info);
                }
            }
        }

        // Chain-wide scan to discover agreements for buckets we don't have locally
        if let Ok(mut iter) = storage_api.iter(storage::all_storage_agreements()).await {
            while let Some(result) = iter.next().await {
                let kv = match result {
                    Ok(kv) => kv,
                    Err(e) => {
                        tracing::debug!("Error iterating storage: {e}");
                        continue;
                    }
                };

                let bucket_id =
                    match Self::extract_bucket_if_provider(&kv.key_bytes, &account_bytes) {
                        Some(id) => id,
                        None => continue,
                    };

                if agreements.iter().any(|a| a.bucket_id == bucket_id) {
                    continue;
                }

                if let Some(info) = Self::to_replica_info(bucket_id, &kv.value) {
                    agreements.push(info);
                }
            }
        }

        Ok(agreements)
    }

    async fn fetch_bucket_snapshot(&self, bucket_id: BucketId) -> Result<BucketSnapshot, Error> {
        let storage_api = self
            .client
            .api()
            .storage()
            .at_latest()
            .await
            .map_err(|e| Error::Internal(format!("Failed to get storage: {e}")))?;

        match storage_api.fetch(&storage::bucket_info(bucket_id)).await {
            Ok(Some(bucket)) => match bucket.snapshot {
                Some(snap) => Ok(BucketSnapshot {
                    mmr_root: snap.mmr_root,
                    leaf_count: snap.leaf_count,
                }),
                None => Ok(BucketSnapshot {
                    mmr_root: H256::zero(),
                    leaf_count: 0,
                }),
            },
            _ => Ok(BucketSnapshot {
                mmr_root: H256::zero(),
                leaf_count: 0,
            }),
        }
    }

    async fn fetch_primary_endpoints(&self, bucket_id: BucketId) -> Result<Vec<String>, Error> {
        let storage_api = self
            .client
            .api()
            .storage()
            .at_latest()
            .await
            .map_err(|e| Error::Internal(format!("Failed to get storage: {e}")))?;

        let bucket = match storage_api.fetch(&storage::bucket_info(bucket_id)).await {
            Ok(Some(b)) => b,
            _ => return Ok(vec![]),
        };

        let mut endpoints = Vec::new();
        for rt_account in bucket.primary_providers.0 {
            let account = AccountId32::from(rt_account.0);
            if let Ok(Some(info)) = storage_api.fetch(&storage::provider_info(&account)).await {
                let multiaddr_str = String::from_utf8_lossy(&info.multiaddr.0);
                endpoints.push(Self::multiaddr_to_http_endpoint(&multiaddr_str));
            }
        }

        Ok(endpoints)
    }

    async fn submit_sync_confirmation(
        &self,
        bucket_id: BucketId,
        target_mmr_root: H256,
    ) -> Result<(u8, u128), Error> {
        let mut roots = [None; 7];
        roots[0] = Some(target_mmr_root);

        let tx = extrinsics::confirm_replica_sync(bucket_id, roots, vec![0u8; 64]);

        tracing::info!(
            "Submitting confirm_replica_sync for bucket {} with root 0x{}",
            bucket_id,
            hex::encode(target_mmr_root.as_bytes())
        );

        let signer = self
            .client
            .signer()
            .map_err(|e| Error::Internal(e.to_string()))?;
        let tx_progress = self
            .client
            .api()
            .tx()
            .sign_and_submit_then_watch_default(&tx, signer)
            .await
            .map_err(|e| Error::Internal(format!("Failed to submit tx: {e}")))?;

        tx_progress
            .wait_for_finalized_success()
            .await
            .map_err(|e| Error::Internal(format!("Transaction failed: {e}")))?;

        tracing::info!(
            "confirm_replica_sync submitted successfully for bucket {}",
            bucket_id
        );

        Ok((0, 0))
    }
}

#[async_trait::async_trait]
impl ChallengeChainClient for SubxtChainClient {
    async fn poll_challenges(&self) -> Result<Vec<DetectedChallenge>, Error> {
        Ok(vec![])
    }

    async fn submit_response(
        &self,
        challenge_id: (u32, u16),
        chunk_data: Vec<u8>,
        mmr_proof: storage_primitives::MmrProof,
        chunk_proof: storage_primitives::MerkleProof,
    ) -> Result<H256, Error> {
        let tx = extrinsics::respond_to_challenge_proof(
            challenge_id,
            &chunk_data,
            &mmr_proof,
            &chunk_proof,
        );

        let signer = self
            .client
            .signer()
            .map_err(|e| Error::Internal(e.to_string()))?;
        let tx_progress = self
            .client
            .api()
            .tx()
            .sign_and_submit_then_watch_default(&tx, signer)
            .await
            .map_err(|e| Error::Internal(format!("Failed to submit tx: {e}")))?;

        tx_progress
            .wait_for_finalized_success()
            .await
            .map_err(|e| Error::Internal(format!("Transaction failed: {e}")))?;

        Ok(H256::zero())
    }
}
