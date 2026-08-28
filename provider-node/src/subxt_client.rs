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

use crate::challenge_responder::{ChallengeChainClient, ChallengeError, DetectedChallenge};
use crate::replica_sync_coordinator::{
    BucketSnapshot, ReplicaAgreementInfo, ReplicaSyncChainClient,
};
use crate::Error;
use provider_chain::chain_connection::{self, ChainWatch};
use sp_core::crypto::Ss58Codec;
use sp_core::H256;
use std::sync::Arc;
use std::time::Duration;
use storage_primitives::{BucketId, ChunkLocation, Commitment};
use storage_subxt::api::runtime_types::bounded_collections::bounded_vec::BoundedVec;
use storage_subxt::api::runtime_types::pallet_storage_provider::pallet::ChallengeResponse as RuntimeChallengeResponse;
use storage_subxt::api::runtime_types::storage_primitives::{
    ChallengeId as RuntimeChallengeId, MerkleProof as RuntimeMerkleProof,
    MmrLeaf as RuntimeMmrLeaf, MmrProof as RuntimeMmrProof,
};
use subxt::dynamic::Value;
use subxt::error::{DispatchError, TransactionEventsError, TransactionFinalizedSuccessError};
use subxt::ext::scale_value::value;

/// `StorageProvider` error variants that mean "this action already happened"
/// — a retry after a dropped transaction watch may race an earlier attempt
/// that landed, and these rejections prove the duty is done rather than
/// failed. Matched against the pallet + error-variant names resolved from
/// runtime metadata (see [`SubxtChainClient::is_already_done`]).
const ALREADY_DONE_ERRORS: [&str; 2] = [
    "ChallengeNotFound", // respond_to_challenge: challenge was taken on defense
    "SyncTooFrequent",   // confirm_replica_sync: a sync already confirmed
];

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
    Retryable(Error),
    /// The chain rejected the call itself; resubmitting would fail identically.
    Rejected(Error),
}

/// Query the pallet's `StorageProviderApi::current_anchor_block` runtime API —
/// the block every on-chain duration (timeouts, expiries, `valid_until`, nonce
/// age) is measured against. Reading it through the runtime API keeps the
/// provider agnostic to whether the anchor is a relay, parachain, or other
/// block number: the pallet decides via its `BlockNumberProvider`, and the
/// provider no longer reaches into a specific storage item.
///
/// Kept here (rather than in `storage-client`) so the provider node stays
/// dependency-light (see #275).
pub(crate) async fn fetch_current_anchor_block<C>(
    at: &subxt::client::ClientAtBlock<subxt::PolkadotConfig, C>,
) -> Result<u32, Error>
where
    C: subxt::client::OnlineClientAtBlockT<subxt::PolkadotConfig>,
{
    use codec::Decode;
    // Invoke by the runtime API's `state_call` name and decode the raw SCALE
    // response directly as the block number. Decoding by hand (rather than
    // through the dynamic value path) avoids depending on this API being
    // present in the node's metadata snapshot.
    let bytes = at
        .runtime_apis()
        .call_raw("StorageProviderApi_current_anchor_block", None)
        .await
        .map_err(|e| {
            Error::Internal(format!("current_anchor_block runtime API call failed: {e}"))
        })?;
    u32::decode(&mut &bytes[..])
        .map_err(|e| Error::Internal(format!("Failed to decode anchor block: {e}")))
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

        Ok(Self {
            chain_rx,
            signer,
            submit_lock: Arc::new(tokio::sync::Mutex::new(())),
        })
    }

    /// The current live connection, or an error while the chain has never
    /// been reached yet.
    fn api(&self) -> Result<subxt::OnlineClient<subxt::PolkadotConfig>, Error> {
        chain_connection::current_api(&self.chain_rx).map_err(Into::into)
    }

    /// Get the current anchor block (the clock every on-chain duration is
    /// measured against), read at the latest finalized state via the pallet's
    /// runtime API.
    ///
    /// Backs `get_current_block` on the replica-sync trait.
    async fn current_anchor_block(&self) -> Result<u64, Error> {
        let at = self
            .api()?
            .at_current_block()
            .await
            .map_err(|e| Error::Internal(format!("Failed to get current block: {e}")))?;
        fetch_current_anchor_block(&at).await.map(u64::from)
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

    /// Whether the dispatch error is one of the pallet's duplicate
    /// rejections ([`ALREADY_DONE_ERRORS`]): on a retry it proves the first
    /// attempt landed and the duty is complete.
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
        details.pallet.name() == "StorageProvider"
            && ALREADY_DONE_ERRORS.contains(&details.variant.name.as_str())
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
        what: &str,
    ) -> Result<Option<H256>, Error> {
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
        what: &str,
        retrying: bool,
    ) -> Attempt {
        match tokio::time::timeout(SUBMIT_TIMEOUT, self.submit_once(tx, what, retrying)).await {
            Ok(attempt) => attempt,
            Err(_) => Attempt::Retryable(Error::Internal(format!(
                "not finalized within {}s",
                SUBMIT_TIMEOUT.as_secs()
            ))),
        }
    }

    /// The single unbounded submit-and-watch pass that [`Self::try_submit`]
    /// puts a deadline on.
    async fn submit_once<C: subxt::tx::Payload>(
        &self,
        tx: &C,
        what: &str,
        retrying: bool,
    ) -> Attempt {
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
            Err(e) if !Self::is_dispatch_failure(&e) => {
                Attempt::Retryable(Error::Internal(format!("tx watch failed: {e}")))
            }
            Err(e) => Attempt::Rejected(Error::Internal(format!("Transaction failed: {e}"))),
        }
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
            Ok(_) => tracing::info!("Multiaddr updated on-chain to: {}", expected_multiaddr),
            Err(e) => tracing::error!("Multiaddr update tx failed: {}", e),
        }
    }

    /// Decode a storage agreement from raw SCALE-encoded bytes via the
    /// generated runtime type, keeping this free of hand-rolled offsets.
    fn decode_storage_agreement_bytes(
        bucket_id: BucketId,
        bytes: &[u8],
    ) -> Result<ReplicaAgreementInfo, Error> {
        use codec::Decode;
        use storage_subxt::api::runtime_types::pallet_storage_provider::pallet::StorageAgreement;
        use storage_subxt::api::runtime_types::storage_primitives::ProviderRole as RuntimeProviderRole;

        let agreement = StorageAgreement::decode(&mut &bytes[..])
            .map_err(|e| Error::Internal(format!("Failed to decode agreement: {e}")))?;
        let RuntimeProviderRole::Replica {
            sync_balance,
            sync_price,
            min_sync_interval,
            last_sync,
        } = agreement.role
        else {
            return Err(Error::Internal("Not a replica agreement".to_string()));
        };

        Ok(ReplicaAgreementInfo {
            bucket_id,
            sync_balance,
            sync_price,
            min_sync_interval: u64::from(min_sync_interval),
            last_sync: last_sync.map(|r| (H256::from(r.root.0), u64::from(r.block))),
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
impl ReplicaSyncChainClient for SubxtChainClient {
    async fn get_current_block(&self) -> Result<u64, Error> {
        self.current_anchor_block().await
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

/// The generated `Challenge` value held in `StorageProvider::Challenges`.
type OnChainChallenge = storage_subxt::api::storage_provider::storage::challenges::Output;

/// `storage-primitives` and the generated bindings carry structurally identical
/// proof types; only the `H256` differs (`sp_core` vs `subxt::utils`).
fn to_runtime_hash(hash: H256) -> subxt::utils::H256 {
    subxt::utils::H256(hash.0)
}

fn to_runtime_merkle_proof(proof: storage_primitives::MerkleProof) -> RuntimeMerkleProof {
    RuntimeMerkleProof {
        siblings: proof.siblings.into_iter().map(to_runtime_hash).collect(),
        path: proof.path,
    }
}

/// Map an on-chain `Challenge` at `(deadline, index)` into the responder's
/// `DetectedChallenge`, or `None` when it targets a different provider.
fn detected_challenge(
    deadline: u32,
    index: u16,
    challenge: OnChainChallenge,
    our_bytes: &[u8; 32],
) -> Option<DetectedChallenge> {
    if &challenge.provider.0 != our_bytes {
        return None;
    }

    Some(DetectedChallenge {
        bucket_id: challenge.bucket_id,
        deadline,
        index,
        commitment: Commitment {
            mmr_root: H256::from(challenge.commitment.mmr_root.0),
            start_seq: challenge.commitment.start_seq,
            leaf_count: challenge.commitment.leaf_count,
        },
        target: ChunkLocation {
            leaf_index: challenge.target.leaf_index,
            chunk_index: challenge.target.chunk_index,
        },
        challenger: sp_core::crypto::AccountId32::from(challenge.challenger.0).to_ss58check(),
    })
}

/// One challenge as returned by the `provider_challenges` runtime API.
type ChallengeFromRuntimeApi =
    storage_subxt::api::runtime_types::pallet_storage_provider::runtime_api::ChallengeResponse;

/// Map a `provider_challenges` entry into a `DetectedChallenge`.
///
/// Carries accounts as SCALE-encoded bytes, so it needs its own mapping. No
/// provider check here: the runtime API filtered on-chain.
fn detected_from_response(challenge: ChallengeFromRuntimeApi) -> DetectedChallenge {
    // An unreadable challenger only costs us a diagnostic field, so it must not
    // discard the challenge — failing to respond is what gets us slashed.
    let challenger = match <[u8; 32]>::try_from(challenge.challenger.as_slice()) {
        Ok(bytes) => sp_core::crypto::AccountId32::from(bytes).to_ss58check(),
        Err(_) => {
            tracing::warn!(
                "Challenge {}/{} carried a {}-byte challenger account, expected 32",
                challenge.deadline,
                challenge.index,
                challenge.challenger.len()
            );
            String::new()
        }
    };

    DetectedChallenge {
        bucket_id: challenge.bucket_id,
        deadline: challenge.deadline,
        index: challenge.index,
        commitment: Commitment {
            mmr_root: H256::from(challenge.commitment.mmr_root.0),
            start_seq: challenge.commitment.start_seq,
            leaf_count: challenge.commitment.leaf_count,
        },
        target: ChunkLocation {
            leaf_index: challenge.target.leaf_index,
            chunk_index: challenge.target.chunk_index,
        },
        challenger,
    }
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
            .api()
            .map_err(|e| ChallengeError::Chain(e.to_string()))?
            .at_current_block()
            .await
            .map_err(|e| ChallengeError::Chain(format!("Failed to get storage: {e}")))?
            .runtime_apis()
            .call(payload)
            .await
            .map_err(|e| {
                ChallengeError::Chain(format!("provider_challenges runtime API call failed: {e}"))
            })?;

        Ok(challenges.into_iter().map(detected_from_response).collect())
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
    ) -> Result<Option<DetectedChallenge>, ChallengeError> {
        let our_bytes: [u8; 32] = self.signer.public_key().0;

        // `unvalidated`: see the `storage-subxt` crate docs.
        let storage_address = storage_subxt::api::storage()
            .storage_provider()
            .challenges()
            .unvalidated();
        let at = self
            .api()
            .map_err(|e| ChallengeError::Chain(e.to_string()))?
            .at_current_block()
            .await
            .map_err(|e| ChallengeError::Chain(format!("Failed to get storage: {e}")))?;

        let Some(value) = at
            .storage()
            .try_fetch(storage_address, (deadline, index))
            .await
            .map_err(|e| ChallengeError::Chain(format!("Failed to fetch challenge: {e}")))?
        else {
            return Ok(None);
        };

        let challenge = value.decode().map_err(|e| {
            ChallengeError::Chain(format!(
                "Failed to decode challenge at {deadline}/{index}: {e}"
            ))
        })?;

        Ok(detected_challenge(deadline, index, challenge, &our_bytes))
    }

    async fn submit_response(
        &self,
        challenge_id: (u32, u16),
        chunk_data: Vec<u8>,
        mmr_proof: storage_primitives::MmrProof,
        chunk_proof: storage_primitives::MerkleProof,
    ) -> Result<H256, ChallengeError> {
        let (deadline, index) = challenge_id;

        let response = RuntimeChallengeResponse::Proof {
            chunk_data: BoundedVec(chunk_data),
            mmr_proof: RuntimeMmrProof {
                peaks: mmr_proof.peaks.into_iter().map(to_runtime_hash).collect(),
                leaf: RuntimeMmrLeaf {
                    data_root: to_runtime_hash(mmr_proof.leaf.data_root),
                    data_size: mmr_proof.leaf.data_size,
                    total_size: mmr_proof.leaf.total_size,
                },
                leaf_proof: to_runtime_merkle_proof(mmr_proof.leaf_proof),
            },
            chunk_proof: to_runtime_merkle_proof(chunk_proof),
        };

        let tx = storage_subxt::api::tx()
            .storage_provider()
            .respond_to_challenge(RuntimeChallengeId { deadline, index }, response);

        // Zero only when success was inferred from a duplicate rejection, where
        // the block that took the response is genuinely unknown.
        let block_hash = self
            .submit_and_finalize(&tx, "respond_to_challenge")
            .await
            .map_err(|e| ChallengeError::Chain(e.to_string()))?;

        Ok(block_hash.unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use storage_subxt::api::runtime_types::storage_primitives::{
        ChunkLocation as RuntimeChunkLocation, Commitment as RuntimeCommitment,
    };

    const OURS: [u8; 32] = [9u8; 32];

    /// Build an on-chain `Challenge` targeting `provider`.
    ///
    /// Constructed from the generated type rather than from hand-rolled SCALE
    /// bytes, so a change to the runtime's `Challenge` shape breaks the build
    /// here instead of silently shifting a byte offset at runtime.
    fn on_chain_challenge(provider: [u8; 32]) -> OnChainChallenge {
        OnChainChallenge {
            bucket_id: 42,
            provider: subxt::utils::AccountId32(provider),
            challenger: subxt::utils::AccountId32([2u8; 32]),
            commitment: RuntimeCommitment {
                mmr_root: subxt::utils::H256([3u8; 32]),
                start_seq: 100,
                leaf_count: 9,
            },
            target: RuntimeChunkLocation {
                leaf_index: 7,
                chunk_index: 5,
            },
            deposit: 1_000_000_000_000,
        }
    }

    #[test]
    fn maps_challenge_targeting_us() {
        let detected = detected_challenge(11, 3, on_chain_challenge(OURS), &OURS)
            .expect("challenge targets us");

        assert_eq!(detected.bucket_id, 42);
        assert_eq!(detected.deadline, 11);
        assert_eq!(detected.index, 3);
        assert_eq!(detected.commitment.mmr_root.0, [3u8; 32]);
        assert_eq!(detected.commitment.start_seq, 100);
        assert_eq!(detected.commitment.leaf_count, 9);
        // Both come from the nested `target: ChunkLocation`, and they are
        // deliberately different values so a swapped mapping is caught.
        assert_eq!(detected.target.leaf_index, 7);
        assert_eq!(detected.target.chunk_index, 5);
        assert_eq!(
            detected.challenger,
            sp_core::crypto::AccountId32::from([2u8; 32]).to_ss58check()
        );
    }

    #[test]
    fn filters_challenge_targeting_another_provider() {
        assert!(
            detected_challenge(11, 3, on_chain_challenge([1u8; 32]), &OURS).is_none(),
            "challenge for another provider is filtered out"
        );
    }

    /// Build a `provider_challenges` response entry with the given challenger
    /// bytes, so the account-decoding path can be exercised both ways.
    fn runtime_api_challenge(challenger: Vec<u8>) -> ChallengeFromRuntimeApi {
        ChallengeFromRuntimeApi {
            bucket_id: 42,
            provider: OURS.to_vec(),
            challenger,
            commitment: RuntimeCommitment {
                mmr_root: subxt::utils::H256([3u8; 32]),
                start_seq: 100,
                leaf_count: 9,
            },
            target: RuntimeChunkLocation {
                leaf_index: 7,
                chunk_index: 5,
            },
            deadline: 11,
            index: 3,
            deposit: 1_000_000_000_000,
        }
    }

    #[test]
    fn maps_runtime_api_challenge() {
        let detected = detected_from_response(runtime_api_challenge([2u8; 32].to_vec()));

        assert_eq!(detected.bucket_id, 42);
        assert_eq!(detected.deadline, 11);
        assert_eq!(detected.index, 3);
        assert_eq!(detected.commitment.mmr_root.0, [3u8; 32]);
        assert_eq!(detected.commitment.leaf_count, 9);
        assert_eq!(detected.target.leaf_index, 7);
        assert_eq!(detected.target.chunk_index, 5);
        assert_eq!(
            detected.challenger,
            sp_core::crypto::AccountId32::from([2u8; 32]).to_ss58check()
        );
    }

    #[test]
    fn malformed_challenger_does_not_discard_the_challenge() {
        let detected = detected_from_response(runtime_api_challenge(vec![7u8; 8]));

        // Only the diagnostic field degrades; the challenge still has to be
        // answered, since not answering is what gets the provider slashed.
        assert!(detected.challenger.is_empty());
        assert_eq!(detected.bucket_id, 42);
        assert_eq!(detected.target.leaf_index, 7);
        assert_eq!(detected.target.chunk_index, 5);
    }
}
