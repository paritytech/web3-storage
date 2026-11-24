// SPDX-License-Identifier: GPL-3.0-only

//! Subxt-based production chain client shared by all coordinators.
//!
//! A single [`SubxtChainClient`] holds one [`subxt::OnlineClient`] connection
//! and one signing key, and implements every coordinator's chain-client trait
//! (`ReplicaSyncChainClient`, `ChallengeChainClient`). Coordinators still
//! depend on the narrow trait they need, so per-trait mocks keep working; the
//! production wiring just hands each one a clone of the same client (a cheap
//! `OnlineClient`/`Keypair` clone that shares the underlying WebSocket
//! connection).

use crate::chain_connection::{self, ChainWatch};
use crate::challenge_responder::{
    ChallengeChainClient, ChallengeError, ChallengeResponse, DetectedChallenge,
};
use crate::runtime_conversions::{
    bind_addr_to_multiaddr, bucket_snapshot, detected_challenge, detected_from_response,
    provider_endpoint, replica_agreements_for_local_buckets, subxt_account, to_runtime_hash,
    to_runtime_mmr_proof, to_runtime_signature,
};
use provider_replica::coordinator::{BucketSnapshot, ReplicaAgreementInfo};
use provider_replica::{ChainClientError, ReplicaSyncChainClient, SignedSyncRoots};
use sp_core::crypto::Ss58Codec;
use sp_core::H256;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use storage_primitives::BucketId;
use storage_subxt::api::runtime_types::bounded_collections::bounded_vec::BoundedVec;
use storage_subxt::api::runtime_types::pallet_storage_provider::types::{
    Bucket as OnChainBucket, ChallengeId as RuntimeChallengeId,
    ChallengeResponse as RuntimeChallengeResponse, ProviderRole,
};
use subxt::error::{DispatchError, TransactionEventsError, TransactionFinalizedSuccessError};

/// Upper bound on one submit-and-watch pass. Generous next to normal
/// finalization (a few blocks) so a slow chain isn't mistaken for a stuck one,
/// but it keeps [`SubxtChainClient::submit_lock`] — held until finalization —
/// from being wedged forever by a watch that neither resolves nor errors.
const SUBMIT_TIMEOUT: Duration = Duration::from_secs(120);

/// Outcome of one submit-and-watch pass (see
/// [`SubxtChainClient::submit_and_finalize`]).
enum Attempt {
    /// The transaction landed, or a duplicate rejection proved an earlier
    /// attempt did. Carries the finalized block hash, except on the
    /// duplicate-rejection path: there we only learn that some earlier
    /// submission landed, never which block took it.
    Landed(Option<H256>),
    /// Transport-level failure: the transaction may or may not have landed, so
    /// resubmitting is safe only because a duplicate rejection counts as
    /// success.
    Retryable(ChainClientError),
    /// The chain rejected the call itself; resubmitting would fail identically.
    Rejected(ChainClientError),
}

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
    /// Serializes transaction submission across all clones. subxt 0.50 reads
    /// the nonce from state at the anchored finalized block (not the pool), so
    /// two concurrent submissions from the same signer would pick the same
    /// nonce and one would be rejected. Held until finalization for the same
    /// reason: the nonce only advances once the first transaction lands.
    // ponytail: per-node serialization; a real nonce manager if duty-tx volume ever grows.
    submit_lock: Arc<tokio::sync::Mutex<()>>,
}

/// Turning the configured seed into a keypair is all [`SubxtChainClient::new`]
/// can fail at, and it happens once at startup: `command.rs` logs it and runs
/// without a chain client. It never reaches a request, so it is deliberately
/// not an HTTP error.
#[derive(Debug, thiserror::Error)]
pub enum SignerSetupError {
    /// The `--keyfile` contents are not a valid seed URI.
    #[error("invalid seed URI: {0}")]
    SecretUri(#[from] subxt_signer::SecretUriError),
    /// The seed parsed but no sr25519 keypair could be derived from it.
    #[error("cannot build keypair from seed: {0}")]
    Keypair(#[from] subxt_signer::sr25519::Error),
}

impl SubxtChainClient {
    /// Create the signing chain client from the connection watch channel and
    /// the provider's seed URI (e.g. `//Alice` or a mnemonic), which
    /// reproduces the provider's registered account — the identity every
    /// on-chain action must be signed by.
    pub fn new(chain_rx: ChainWatch, seed: &str) -> Result<Self, SignerSetupError> {
        let uri: subxt_signer::SecretUri = seed.parse()?;
        let signer = subxt_signer::sr25519::Keypair::from_uri(&uri)?;

        tracing::info!(
            "Chain client signing as {}",
            sp_core::crypto::AccountId32::from(signer.public_key().0).to_ss58check()
        );

        Ok(Self {
            chain_rx,
            signer,
            submit_lock: Arc::new(tokio::sync::Mutex::new(())),
        })
    }

    /// The current live connection, or an error while the chain has never
    /// been reached yet.
    fn api(
        &self,
    ) -> Result<subxt::OnlineClient<subxt::PolkadotConfig>, chain_connection::NotConnected> {
        chain_connection::current_api(&self.chain_rx)
    }

    /// The block handle every chain read starts from: the live connection,
    /// resolved to the current block. Both steps report the same failure, so
    /// the mapping lives here rather than at each call site.
    async fn at_current_block(
        &self,
    ) -> Result<
        subxt::client::ClientAtBlock<
            subxt::PolkadotConfig,
            subxt::client::OnlineClientAtBlockImpl<subxt::PolkadotConfig>,
        >,
        ChainClientError,
    > {
        self.api()
            .map_err(|e| ChainClientError::query("chain connection", e))?
            .at_current_block()
            .await
            .map_err(|e| ChainClientError::query("current block", e))
    }

    /// Get the current anchor block (the clock every on-chain duration is
    /// measured against), read at the latest finalized state via the pallet's
    /// runtime API.
    ///
    /// Backs `get_current_block` on the replica-sync trait.
    async fn current_anchor_block(&self) -> Result<u64, ChainClientError> {
        let at = self.at_current_block().await?;
        Ok(u64::from(fetch_current_anchor_block(&at).await?))
    }

    /// Whether the failure is the chain rejecting the call itself (a
    /// dispatch error decoded from `System::ExtrinsicFailed`), as opposed to
    /// the watch or transport dying before a verdict was seen.
    fn is_dispatch_failure(e: &TransactionFinalizedSuccessError) -> bool {
        matches!(
            e,
            TransactionFinalizedSuccessError::SuccessError(
                TransactionEventsError::ExtrinsicFailed(_)
            )
        )
    }

    /// Whether the dispatch error is `StorageProvider::SyncTooFrequent`:
    /// `confirm_replica_sync` rejects a second sync within
    /// `min_sync_interval`, so on a retry it proves the first attempt landed
    /// and the duty is complete.
    fn is_already_done(e: &TransactionFinalizedSuccessError) -> bool {
        let TransactionFinalizedSuccessError::SuccessError(
            TransactionEventsError::ExtrinsicFailed(DispatchError::Module(module_error)),
        ) = e
        else {
            return false;
        };
        let Ok(details) = module_error.details() else {
            return false;
        };
        details.pallet.name() == "StorageProvider" && details.variant.name == "SyncTooFrequent"
    }

    /// Sign, submit, and wait for finalized success, retrying once on
    /// transport-level failures (dropped socket, backend resubscription
    /// killing the transaction watch).
    ///
    /// On the retry, a duplicate rejection ([`Self::is_already_done`]) is
    /// treated as success: it means the first submission actually landed and
    /// the duty is complete.
    ///
    /// Returns the finalized block hash, or `None` when success was inferred
    /// from a duplicate rejection rather than observed directly.
    async fn submit_and_finalize<C: subxt::tx::Payload>(
        &self,
        tx: &C,
        what: &'static str,
    ) -> Result<Option<H256>, ChainClientError> {
        const RETRY_DELAY: Duration = Duration::from_secs(6);

        // One transaction at a time across every clone (see `submit_lock`).
        let _guard = self.submit_lock.lock().await;

        let first_failure = match self.try_submit(tx, what, false).await {
            Attempt::Landed(block_hash) => return Ok(block_hash),
            Attempt::Rejected(e) => return Err(e),
            Attempt::Retryable(e) => e,
        };

        tracing::warn!("{what}: {first_failure}; retrying once");
        tokio::time::sleep(RETRY_DELAY).await;

        match self.try_submit(tx, what, true).await {
            Attempt::Landed(block_hash) => Ok(block_hash),
            Attempt::Retryable(e) | Attempt::Rejected(e) => Err(e),
        }
    }

    /// One submit-and-watch pass, bounded by [`SUBMIT_TIMEOUT`]. `retrying`
    /// marks the second attempt, where a duplicate rejection proves the first
    /// one landed.
    ///
    /// A timed-out pass is [`Attempt::Retryable`], so a transaction that did
    /// land while the watch hung is still recognised by the duplicate-rejection
    /// path on the resubmit. With both passes bounded, the total `submit_lock`
    /// hold is at most `2 * SUBMIT_TIMEOUT + RETRY_DELAY`.
    async fn try_submit<C: subxt::tx::Payload>(
        &self,
        tx: &C,
        what: &'static str,
        retrying: bool,
    ) -> Attempt {
        match tokio::time::timeout(SUBMIT_TIMEOUT, self.submit_once(tx, what, retrying)).await {
            Ok(attempt) => attempt,
            Err(_) => Attempt::Retryable(ChainClientError::tx_submit(
                what,
                format!("not finalized within {}s", SUBMIT_TIMEOUT.as_secs()),
            )),
        }
    }

    /// The single unbounded submit-and-watch pass that [`Self::try_submit`]
    /// puts a deadline on.
    async fn submit_once<C: subxt::tx::Payload>(
        &self,
        tx: &C,
        what: &'static str,
        retrying: bool,
    ) -> Attempt {
        let submitted = async {
            self.at_current_block()
                .await?
                .transactions()
                .sign_and_submit_then_watch_default(tx, &self.signer)
                .await
                .map_err(|e| ChainClientError::tx_submit(what, e))
        }
        .await;

        let progress = match submitted {
            Ok(progress) => progress,
            Err(e) => return Attempt::Retryable(e),
        };

        // `wait_for_finalized_success` is split into its two stages so the block
        // hash is available before the dispatch outcome is known; both stage
        // errors convert into the combined type the classifiers below match on.
        let finalized = async {
            let in_block = progress.wait_for_finalized().await?;
            let block_hash = in_block.block_hash();
            in_block.wait_for_success().await?;
            Ok::<_, TransactionFinalizedSuccessError>(block_hash)
        }
        .await;

        match finalized {
            Ok(block_hash) => Attempt::Landed(Some(H256::from(block_hash.0))),
            Err(e) if retrying && Self::is_already_done(&e) => {
                tracing::info!("{what}: duplicate rejected on retry ({e}); first attempt landed");
                Attempt::Landed(None)
            }
            // Non-dispatch failure (e.g. the watch subscription died): the tx
            // may or may not have landed, so resubmit and let the duplicate
            // classification above decide.
            Err(e) if !Self::is_dispatch_failure(&e) => Attempt::Retryable(
                ChainClientError::tx_submit(what, format!("tx watch failed: {e}")),
            ),
            Err(e) => Attempt::Rejected(ChainClientError::tx_rejected(what, e)),
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
        let expected_multiaddr = match public_multiaddr {
            Some(addr) => addr.to_string(),
            None => bind_addr_to_multiaddr(bind_addr),
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
        let storage_query = storage_subxt::api::storage()
            .storage_provider()
            .providers()
            .unvalidated();

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
            .try_fetch(storage_query, (subxt_account(&our_account),))
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

        let provider = match provider_value.decode() {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("Could not decode provider value: {}, skipping sync", e);
                return;
            }
        };
        let current = String::from_utf8_lossy(&provider.multiaddr.0).to_string();

        if provider.multiaddr.0 == expected_multiaddr.as_bytes() {
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

        let tx = storage_subxt::api::tx()
            .storage_provider()
            .update_provider_multiaddr(BoundedVec(expected_multiaddr.as_bytes().to_vec()));

        match self
            .submit_and_finalize(&tx, "update_provider_multiaddr")
            .await
        {
            Ok(_) => tracing::info!("Multiaddr updated on-chain to: {}", expected_multiaddr),
            Err(e) => tracing::error!("Multiaddr update tx failed: {}", e),
        }
    }

    /// Resolve each account to its HTTP endpoint from its on-chain multiaddr.
    /// Skips accounts with no registration or an empty multiaddr.
    async fn endpoints_for_accounts(
        &self,
        accounts: Vec<subxt::utils::AccountId32>,
    ) -> Result<Vec<String>, ChainClientError> {
        if accounts.is_empty() {
            return Ok(Vec::new());
        }
        let at = self.at_current_block().await?;
        let mut endpoints = Vec::new();
        for account in accounts {
            let providers = storage_subxt::api::storage()
                .storage_provider()
                .providers()
                .unvalidated();
            let Some(value) = at
                .storage()
                .try_fetch(providers, (account,))
                .await
                .map_err(|e| ChainClientError::query("provider", e))?
            else {
                continue;
            };
            let provider = value
                .decode()
                .map_err(|e| ChainClientError::decode("provider", e))?;
            endpoints.extend(provider_endpoint(&provider));
        }

        Ok(endpoints)
    }

    /// The `StorageProvider::Buckets` entry for `bucket_id`, or `None` when the
    /// bucket does not exist.
    async fn fetch_bucket(
        &self,
        bucket_id: BucketId,
    ) -> Result<Option<OnChainBucket>, ChainClientError> {
        let buckets = storage_subxt::api::storage()
            .storage_provider()
            .buckets()
            .unvalidated();
        self.at_current_block()
            .await?
            .storage()
            .try_fetch(buckets, (bucket_id,))
            .await
            .map_err(|e| ChainClientError::query("bucket", e))?
            .map(|value| value.decode())
            .transpose()
            .map_err(|e| ChainClientError::decode("bucket", e))
    }
}

#[async_trait::async_trait]
impl ReplicaSyncChainClient for SubxtChainClient {
    async fn get_current_block(&self) -> Result<u64, ChainClientError> {
        self.current_anchor_block().await
    }

    /// This provider's replica agreements for the buckets it stores locally.
    ///
    /// Calls the pallet's `StorageProviderApi::provider_agreements` runtime
    /// API, which filters the `StorageAgreements` map to this provider
    /// on-chain, then keeps the `Replica` roles whose bucket appears in
    /// `local_buckets` — syncing a bucket with no local storage behind it has
    /// nothing to sync into.
    async fn fetch_replica_agreements(
        &self,
        provider_account: &str,
        local_buckets: Vec<BucketId>,
    ) -> Result<Vec<ReplicaAgreementInfo>, ChainClientError> {
        // Accepts both SS58 and 0x-prefixed hex, the two forms `--provider-id`
        // is given in.
        let account = sp_core::crypto::AccountId32::from_str(provider_account)
            .map_err(|e| ChainClientError::invalid_account(provider_account, e))?;

        let payload = storage_subxt::api::runtime_apis()
            .storage_provider_api()
            .provider_agreements(subxt_account(&account))
            .unvalidated();

        let agreements = self
            .at_current_block()
            .await?
            .runtime_apis()
            .call(payload)
            .await
            .map_err(|e| ChainClientError::query("provider agreements", e))?;

        Ok(replica_agreements_for_local_buckets(
            agreements,
            local_buckets,
        ))
    }

    async fn fetch_bucket_snapshot(
        &self,
        bucket_id: BucketId,
    ) -> Result<BucketSnapshot, ChainClientError> {
        Ok(bucket_snapshot(self.fetch_bucket(bucket_id).await?))
    }

    async fn fetch_primary_endpoints(
        &self,
        bucket_id: BucketId,
    ) -> Result<Vec<String>, ChainClientError> {
        let primaries = self
            .fetch_bucket(bucket_id)
            .await?
            .map(|bucket| bucket.primary_providers.0)
            .unwrap_or_default();
        self.endpoints_for_accounts(primaries).await
    }

    async fn fetch_replica_endpoints(
        &self,
        bucket_id: BucketId,
    ) -> Result<Vec<String>, ChainClientError> {
        let provider_account = self.signer.public_key().0.to_vec();

        let payload = storage_subxt::api::runtime_apis()
            .storage_provider_api()
            .bucket_agreements(bucket_id)
            .unvalidated();

        let agreements = self
            .at_current_block()
            .await?
            .runtime_apis()
            .call(payload)
            .await
            .map_err(|e| ChainClientError::query("bucket agreements", e))?;

        let accounts: Vec<subxt::utils::AccountId32> = agreements
            .into_iter()
            .filter(|agreement| matches!(agreement.role, ProviderRole::Replica { .. }))
            .map(|agreement| agreement.provider)
            .filter(|provider| provider != &provider_account)
            .filter_map(|provider| <[u8; 32]>::try_from(provider).ok())
            .map(subxt::utils::AccountId32)
            .collect();

        self.endpoints_for_accounts(accounts).await
    }

    async fn submit_sync_confirmation(
        &self,
        bucket_id: BucketId,
        attestation: SignedSyncRoots,
    ) -> Result<(u8, u128), ChainClientError> {
        let tx = storage_subxt::api::tx()
            .storage_provider()
            .confirm_replica_sync(
                bucket_id,
                attestation.roots.map(|root| root.map(to_runtime_hash)),
                to_runtime_signature(&attestation.signature),
            );

        tracing::info!(
            "Submitting confirm_replica_sync for bucket {} with roots {:?}",
            bucket_id,
            attestation.roots
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

/// Query the pallet's `StorageProviderApi::current_anchor_block` runtime API —
/// the block every on-chain duration (timeouts, expiries, `valid_until`) is
/// measured against. Reading it through the runtime API keeps the
/// provider agnostic to whether the anchor is a relay, parachain, or other
/// block number: the pallet decides via its `BlockNumberProvider`.
pub(crate) async fn fetch_current_anchor_block<C>(
    at: &subxt::client::ClientAtBlock<subxt::PolkadotConfig, C>,
) -> Result<u32, ChainClientError>
where
    C: subxt::client::OnlineClientAtBlockT<subxt::PolkadotConfig>,
{
    at.runtime_apis()
        .call(
            storage_subxt::api::runtime_apis()
                .storage_provider_api()
                .current_anchor_block()
                .unvalidated(),
        )
        .await
        .map_err(|e| ChainClientError::query("current anchor block", e))
}

#[async_trait::async_trait]
impl ChallengeChainClient for SubxtChainClient {
    /// Poll for active challenges against this provider.
    ///
    /// Calls the pallet's `StorageProviderApi::provider_challenges` runtime API,
    /// which filters the `Challenges` map to this provider on-chain. That makes
    /// this one round trip returning only actionable challenges, rather than an
    /// iteration over every unexpired deadline filtered client-side.
    async fn poll_challenges(&self) -> Result<Vec<DetectedChallenge>, ChallengeError> {
        let payload = storage_subxt::api::runtime_apis()
            .storage_provider_api()
            .provider_challenges(subxt::utils::AccountId32(self.signer.public_key().0))
            .unvalidated();

        let challenges = self
            .at_current_block()
            .await
            .map_err(|e| ChallengeError::Chain {
                reason: e.to_string(),
            })?
            .runtime_apis()
            .call(payload)
            .await
            .map_err(|e| ChallengeError::Chain {
                reason: format!("provider_challenges runtime API call failed: {e}"),
            })?;

        Ok(challenges.into_iter().map(detected_from_response).collect())
    }

    /// Point-read a single challenge at `(deadline, index)`.
    ///
    /// Backs the event-driven path: a `ChallengeCreated` event carries the
    /// challenge id but not the proof parameters, so the responder fetches
    /// the full `Challenge` value here. Returns `None` when the entry is
    /// missing (already responded / resolved) or targets another provider.
    async fn fetch_challenge(
        &self,
        deadline: u32,
        index: u16,
    ) -> Result<Option<DetectedChallenge>, ChallengeError> {
        let our_bytes: [u8; 32] = self.signer.public_key().0;

        let storage_address = storage_subxt::api::storage()
            .storage_provider()
            .challenges()
            .unvalidated();
        let at = self
            .at_current_block()
            .await
            .map_err(|e| ChallengeError::Chain {
                reason: e.to_string(),
            })?;

        let Some(value) = at
            .storage()
            .try_fetch(storage_address, (deadline, index))
            .await
            .map_err(|e| ChallengeError::Chain {
                reason: format!("Failed to fetch challenge: {e}"),
            })?
        else {
            return Ok(None);
        };

        // The chain answered, but the value it returned couldn't be decoded.
        let challenge = value.decode().map_err(|e| ChallengeError::Chain {
            reason: format!("Failed to decode challenge at {deadline}/{index}: {e}"),
        })?;

        Ok(detected_challenge(deadline, index, challenge, &our_bytes))
    }

    async fn submit_response(
        &self,
        challenge_id: (u32, u16),
        response: ChallengeResponse,
    ) -> Result<H256, ChallengeError> {
        let (deadline, index) = challenge_id;
        let response = match response {
            ChallengeResponse::Proof {
                chunk_data,
                mmr_proof,
                chunk_proof,
            } => RuntimeChallengeResponse::Proof {
                chunk_data: BoundedVec(chunk_data),
                mmr_proof: to_runtime_mmr_proof(mmr_proof),
                chunk_proof: chunk_proof.into_iter().map(to_runtime_hash).collect(),
            },
            ChallengeResponse::ChunkOutOfRange { mmr_proof } => {
                RuntimeChallengeResponse::ChunkOutOfRange {
                    mmr_proof: to_runtime_mmr_proof(mmr_proof),
                }
            }
        };

        let tx = storage_subxt::api::tx()
            .storage_provider()
            .respond_to_challenge(RuntimeChallengeId { deadline, index }, response);

        // Zero only when success was inferred from a duplicate rejection, where
        // the block that took the response is genuinely unknown.
        let block_hash = self
            .submit_and_finalize(&tx, "respond_to_challenge")
            .await
            .map_err(|e| ChallengeError::Chain {
                reason: e.to_string(),
            })?;

        Ok(block_hash.unwrap_or_default())
    }
}
