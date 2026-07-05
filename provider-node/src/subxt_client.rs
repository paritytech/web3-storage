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
use storage_client::substrate::{extrinsics, storage, SubstrateClient};
use storage_primitives::{BucketId, Commitment, ReplicaSyncRecord};
use storage_subxt::api::runtime_types as rt;
use storage_subxt::subxt::utils::AccountId32;
use storage_subxt::subxt::utils::H256;
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
            AccountId32::from(keypair.public_key().0).to_string()
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
                last_sync: last_sync.as_ref().map(|r| ReplicaSyncRecord {
                    commitment: storage_primitives::Commitment {
                        mmr_root: r.commitment.mmr_root,
                        start_seq: r.commitment.start_seq,
                        leaf_count: r.commitment.leaf_count,
                    },
                    block: r.block as u64,
                }),
            }),
            rt::storage_primitives::ProviderRole::Primary => None,
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

        let commitment = Commitment {
            mmr_root: duty.mmr_root,
            start_seq: duty.start_seq,
            leaf_count: duty.leaf_count,
        };
        let tx = extrinsics::provider_checkpoint(duty.bucket_id, commitment, duty.window, sig_vec);

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
        let account = SubstrateClient::parse_account(provider_account)
            .map_err(|e| Error::Internal(format!("Invalid provider account: {e}")))?;

        let mut agreements = Vec::new();

        let storage_api = self
            .client
            .api()
            .storage()
            .at_latest()
            .await
            .map_err(|e| Error::Internal(format!("Failed to get storage: {e}")))?;

        // Fast path: direct queries for locally-known buckets
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

        // Chain-wide scan via runtime API to discover agreements for buckets we don't have locally
        match self
            .client
            .api()
            .runtime_api()
            .at_latest()
            .await
            .map_err(|e| Error::Internal(format!("runtime api: {e}")))?
            .call(
                storage_subxt::api::apis()
                    .storage_provider_api()
                    .provider_agreements(account),
            )
            .await
        {
            Ok(all) => {
                for a in all {
                    if agreements.iter().any(|x| x.bucket_id == a.bucket_id) {
                        continue;
                    }
                    if let rt::storage_primitives::ProviderRole::Replica {
                        sync_balance,
                        sync_price,
                        min_sync_interval,
                        last_sync,
                    } = a.role
                    {
                        agreements.push(ReplicaAgreementInfo {
                            bucket_id: a.bucket_id,
                            sync_balance,
                            sync_price,
                            min_sync_interval: min_sync_interval as u64,
                            last_sync: last_sync.map(|r| ReplicaSyncRecord {
                                commitment: storage_primitives::Commitment {
                                    mmr_root: r.commitment.mmr_root,
                                    start_seq: r.commitment.start_seq,
                                    leaf_count: r.commitment.leaf_count,
                                },
                                block: r.block as u64,
                            }),
                        });
                    }
                }
            }
            Err(e) => {
                tracing::debug!("provider_agreements runtime API error: {e}");
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
                    mmr_root: snap.commitment.mmr_root,
                    leaf_count: snap.commitment.leaf_count,
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
    /// Poll for active challenges against this provider.
    ///
    /// Delegates to the `StorageProviderApi::provider_challenges` runtime API,
    /// which already scans and filters `StorageProvider::Challenges` on the
    /// node side and returns only the challenges targeting the given account.
    async fn poll_challenges(&self) -> Result<Vec<DetectedChallenge>, Error> {
        let our_account: AccountId32 = self
            .client
            .signer()
            .map_err(|e| Error::Internal(e.to_string()))?
            .public_key()
            .0
            .into();

        let challenges = self
            .client
            .api()
            .runtime_api()
            .at_latest()
            .await
            .map_err(|e| Error::Internal(format!("runtime api: {e}")))?
            .call(
                storage_subxt::api::apis()
                    .storage_provider_api()
                    .provider_challenges(our_account),
            )
            .await
            .map_err(|e| Error::Internal(format!("Failed to fetch provider_challenges: {e}")))?;

        tracing::debug!(
            "Detected {} challenges for current provider",
            challenges.len()
        );

        Ok(challenges
            .into_iter()
            .map(|c| {
                let mut challenger = [0u8; 32];
                challenger.copy_from_slice(&c.challenger);
                DetectedChallenge {
                    bucket_id: c.bucket_id,
                    deadline: c.deadline,
                    index: c.index,
                    mmr_root: c.mmr_root,
                    start_seq: c.start_seq,
                    leaf_index: c.leaf_index,
                    chunk_index: c.chunk_index,
                    challenger: sp_core::crypto::AccountId32::from(challenger).to_ss58check(),
                }
            })
            .collect())
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
