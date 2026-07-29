// SPDX-License-Identifier: GPL-3.0-only

//! # Storage Provider Node
//!
//! Off-chain provider node for scalable Web3 storage.
//!
//! This node provides HTTP APIs for:
//! - Uploading and downloading content-addressed chunks
//! - Committing data to the bucket's MMR
//! - Syncing data between providers (for replicas)

pub mod api;
pub mod chain_connection;
pub mod chain_events;
pub mod chain_state_coordinator;
pub mod challenge_responder;
pub mod cli;
pub mod command;
pub mod error;
pub mod fs_api;
pub mod negotiate;
pub mod replica_sync;
pub mod replica_sync_coordinator;
pub mod s3_api;
pub(crate) mod subxt_client;
pub mod types;

pub use api::create_router;
pub use chain_state_coordinator::{
    is_relevant_provider_event, refresh_if_relevant_event, refresh_provider_state, sync_constants,
    ChainState, ChainStateChainClient, ChainStateCoordinator, ChainStateCoordinatorHandle,
    PalletConstants, ProviderLifecycleEvent,
};
pub use challenge_responder::{
    ChallengeChainClient, ChallengeResponder, ChallengeResponderConfig, ChallengeResponderHandle,
    ChallengeResponseResult, DetectedChallenge, ResponderCommand,
};
pub use error::Error;
pub use negotiate::{AgreementTermsOf, NegotiateRequest, NonceCounter, SignedTerms};
pub use replica_sync::ReplicaSync;
pub use replica_sync_coordinator::{
    ReplicaSyncChainClient, ReplicaSyncCoordinator, ReplicaSyncCoordinatorConfig,
    ReplicaSyncCoordinatorHandle, SyncCommand, SyncCoordinatorStatus, SyncDuty, SyncResult,
};
pub use types::*;

use provider_storage::{FsIndexManager, NonceStore, S3IndexManager, StorageBackend};
use sp_core::crypto::Ss58Codec;
use sp_core::{sr25519, Pair};
use std::sync::Arc;
use std::time::Duration;

/// Everything a servable [`ProviderState`] requires.
pub struct ProviderDeps {
    /// Local storage backend.
    pub storage: Arc<dyn StorageBackend>,
    /// Persistence backing for the nonce counter — [`NullNonceStore`] in
    /// in-memory mode, the disk store in disk mode.
    pub nonce_store: Arc<dyn NonceStore>,
    /// Membership cache for role lookups.
    pub membership: Arc<provider_auth::MembershipCache>,
    /// Maximum allowed clock skew for request timestamps.
    pub auth_max_skew: Duration,
}

/// Provider node state shared across handlers.
pub struct ProviderState {
    /// Local storage backend
    pub storage: Arc<dyn StorageBackend>,
    /// Provider account ID (SS58 encoded)
    pub provider_id: String,
    /// Signing keypair (optional, for dev/testing)
    pub keypair: Option<sr25519::Pair>,
    /// S3-compatible object index
    pub s3_index: S3IndexManager,
    /// File system drive index
    pub fs_index: FsIndexManager,
    /// Membership cache for role lookups.
    pub membership_cache: Arc<provider_auth::MembershipCache>,
    /// Maximum allowed clock skew for request timestamps.
    pub auth_max_skew: Duration,
    /// Browser origins allowed via CORS. `None` (the default) keeps the
    /// permissive policy; `Some(list)` restricts to exactly those origins.
    pub cors_allowed_origins: Option<Vec<String>>,
    /// Live chain state kept in sync by the chain-state coordinator — the single
    /// writer for `current_anchor_block`, `constants`, `provider_info`, and
    /// `nonce_counter`. `/negotiate` gates on all four before signing.
    pub chain_state: Arc<ChainState>,
}

impl ProviderState {
    /// Shared constructor body for [`with_provider_id`](Self::with_provider_id)
    /// and [`with_seed`](Self::with_seed).
    fn from_parts(deps: ProviderDeps, provider_id: String, keypair: Option<sr25519::Pair>) -> Self {
        let ProviderDeps {
            storage,
            nonce_store,
            membership,
            auth_max_skew,
        } = deps;
        Self {
            storage,
            provider_id,
            keypair,
            s3_index: S3IndexManager::new(),
            fs_index: FsIndexManager::new(),
            membership_cache: membership,
            auth_max_skew,
            cors_allowed_origins: None,
            chain_state: Arc::new(ChainState::with_nonce_store(nonce_store)),
        }
    }

    /// Create state for a provider that cannot sign: `provider_id` is used as-is
    /// for identity and on-chain reconciliation, and signing endpoints stay
    /// unavailable. For a signing provider use [`with_seed`](Self::with_seed).
    pub fn with_provider_id(deps: ProviderDeps, provider_id: String) -> Self {
        Self::from_parts(deps, provider_id, None)
    }

    /// Create with a seed phrase or derivation path (e.g., "//Alice", "//Bob").
    pub fn with_seed(deps: ProviderDeps, seed: &str) -> Result<Self, String> {
        let keypair = sr25519::Pair::from_string(seed, None)
            .map_err(|e| format!("Failed to create keypair: {e:?}"))?;

        let provider_id = keypair.public().to_ss58check();

        Ok(Self::from_parts(deps, provider_id, Some(keypair)))
    }

    /// Restrict the browser origins allowed via CORS. `None` (the default) keeps
    /// the permissive policy; `Some(list)` restricts to exactly those origins.
    pub fn with_cors_origins(mut self, origins: Option<Vec<String>>) -> Self {
        self.cors_allowed_origins = origins;
        self
    }

    /// Sign a message and return the signature as `0x`-prefixed hex.
    ///
    /// Returns `Err(Error::SigningUnavailable)` if no keypair is configured.
    /// Callers must propagate this so the HTTP layer returns 503 rather than
    /// silently emitting a 64-zero-byte placeholder signature, which would
    /// be a cryptographically invalid commitment masquerading as a real one.
    pub fn sign(&self, message: &[u8]) -> Result<String, Error> {
        let keypair = self.keypair.as_ref().ok_or(Error::SigningUnavailable)?;
        let signature = keypair.sign(message);
        Ok(format!("0x{}", hex::encode(signature.0)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use provider_storage::NullNonceStore;

    #[test]
    fn sign_without_keypair_refuses_with_signing_unavailable() {
        // The pre-fix behaviour silently returned 64 zero bytes. The new
        // contract is that `sign()` MUST return `Err(SigningUnavailable)`
        // when no keypair is configured, so the HTTP layer can map it to a
        // 503 instead of emitting a cryptographically invalid placeholder.
        let deps = ProviderDeps {
            storage: Arc::new(provider_storage::Storage::new()),
            nonce_store: Arc::new(NullNonceStore),
            membership: Arc::new(provider_auth::MembershipCache::new(
                Box::new(provider_auth::StaticMembershipResolver(vec![])),
                Duration::from_secs(60),
            )),
            auth_max_skew: Duration::from_secs(300),
        };
        let state = ProviderState::with_provider_id(deps, "no-key-provider".to_string());
        let err = state
            .sign(b"any message")
            .expect_err("must refuse to sign without a keypair");
        assert!(matches!(err, Error::SigningUnavailable));
    }

    #[test]
    fn sign_with_keypair_returns_real_sr25519_signature() {
        // Round-trip: sign with //Alice, decode the hex, verify against
        // Alice's public key. This catches any regression where sign() ever
        // returns the 0x00..00 placeholder again, and also catches the more
        // subtle case where the bytes look random but aren't valid sr25519.
        let deps = ProviderDeps {
            storage: Arc::new(provider_storage::Storage::new()),
            nonce_store: Arc::new(NullNonceStore),
            membership: Arc::new(provider_auth::MembershipCache::new(
                Box::new(provider_auth::StaticMembershipResolver(vec![])),
                Duration::from_secs(60),
            )),
            auth_max_skew: Duration::from_secs(300),
        };
        let state = ProviderState::with_seed(deps, "//Alice").unwrap();
        let message = b"commitment-payload-bytes";

        let sig_hex = state.sign(message).expect("signing succeeds with keypair");
        let sig_bytes = hex::decode(sig_hex.strip_prefix("0x").unwrap()).unwrap();
        assert_eq!(sig_bytes.len(), 64, "sr25519 signatures are 64 bytes");
        assert_ne!(
            sig_bytes,
            vec![0u8; 64],
            "must not return the zero-byte placeholder"
        );

        let sig_array: [u8; 64] = sig_bytes
            .clone()
            .try_into()
            .expect("sr25519 signatures are 64 bytes");
        let sig = sr25519::Signature::from_raw(sig_array);
        let alice = keypair_for("//Alice");
        assert!(
            sr25519::Pair::verify(&sig, message, &alice.public()),
            "signature did not verify under //Alice's public key"
        );
    }

    /// Decode an `0x`-prefixed hex signature into an `sr25519::Signature`.
    fn sig_from_hex(sig_hex: &str) -> sr25519::Signature {
        let bytes = hex::decode(sig_hex.strip_prefix("0x").unwrap()).unwrap();
        let array: [u8; 64] = bytes.try_into().expect("sr25519 signatures are 64 bytes");
        sr25519::Signature::from_raw(array)
    }

    /// Derive a keypair from a SURI like `//Alice`.
    fn keypair_for(seed: &str) -> sr25519::Pair {
        sr25519::Pair::from_string(seed, None).unwrap()
    }

    #[test]
    fn sign_produces_distinct_signatures_each_call_but_all_verify() {
        // sr25519 (schnorrkel) is randomised — two calls over the same
        // message produce different signatures, but both must verify. This
        // test guards against accidentally swapping to a backend that
        // returns a constant value (e.g. zero bytes).
        let deps = ProviderDeps {
            storage: Arc::new(provider_storage::Storage::new()),
            nonce_store: Arc::new(NullNonceStore),
            membership: Arc::new(provider_auth::MembershipCache::new(
                Box::new(provider_auth::StaticMembershipResolver(vec![])),
                Duration::from_secs(60),
            )),
            auth_max_skew: Duration::from_secs(300),
        };
        let state = ProviderState::with_seed(deps, "//Alice").unwrap();
        let alice_pub = keypair_for("//Alice").public();
        let msg = b"commitment-payload";

        let sig_a = state.sign(msg).unwrap();
        let sig_b = state.sign(msg).unwrap();

        for sig_hex in [&sig_a, &sig_b] {
            let bytes = hex::decode(sig_hex.strip_prefix("0x").unwrap()).unwrap();
            assert_ne!(bytes, vec![0u8; 64]);
            let sig = sig_from_hex(sig_hex);
            assert!(sr25519::Pair::verify(&sig, msg, &alice_pub));
        }
    }

    #[test]
    fn signatures_from_different_keys_do_not_cross_verify() {
        // Negative control: //Bob's signature must NOT verify under //Alice.
        // Cheap protection against a future refactor that accidentally
        // stops checking the message or the key.
        let alice_deps = ProviderDeps {
            storage: Arc::new(provider_storage::Storage::new()),
            nonce_store: Arc::new(NullNonceStore),
            membership: Arc::new(provider_auth::MembershipCache::new(
                Box::new(provider_auth::StaticMembershipResolver(vec![])),
                Duration::from_secs(60),
            )),
            auth_max_skew: Duration::from_secs(300),
        };
        let bob_deps = ProviderDeps {
            storage: Arc::new(provider_storage::Storage::new()),
            nonce_store: Arc::new(NullNonceStore),
            membership: Arc::new(provider_auth::MembershipCache::new(
                Box::new(provider_auth::StaticMembershipResolver(vec![])),
                Duration::from_secs(60),
            )),
            auth_max_skew: Duration::from_secs(300),
        };
        let alice = ProviderState::with_seed(alice_deps, "//Alice").unwrap();
        let bob = ProviderState::with_seed(bob_deps, "//Bob").unwrap();
        let alice_pub = keypair_for("//Alice").public();
        let msg = b"checkpoint payload";

        let bob_sig = sig_from_hex(&bob.sign(msg).unwrap());
        assert!(!sr25519::Pair::verify(&bob_sig, msg, &alice_pub));

        // Sanity: //Alice's own signature still verifies under her own key.
        let alice_sig = sig_from_hex(&alice.sign(msg).unwrap());
        assert!(sr25519::Pair::verify(&alice_sig, msg, &alice_pub));
    }

    #[test]
    fn provider_state_chain_defaults_on_new() {
        use std::sync::atomic::Ordering;
        let deps = ProviderDeps {
            storage: Arc::new(provider_storage::Storage::new()),
            nonce_store: Arc::new(NullNonceStore),
            membership: Arc::new(provider_auth::MembershipCache::new(
                Box::new(provider_auth::StaticMembershipResolver(vec![])),
                Duration::from_secs(60),
            )),
            auth_max_skew: Duration::from_secs(300),
        };
        let state = ProviderState::with_provider_id(deps, "test-provider".to_string());
        assert_eq!(
            state
                .chain_state
                .current_anchor_block
                .load(Ordering::Relaxed),
            0
        );
        assert!(state.chain_state.provider_info.read().is_none());
    }
}
