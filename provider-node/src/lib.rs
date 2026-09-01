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
pub mod challenge_proofs;
pub mod cli;
pub mod command;
pub mod error;
pub mod fs_api;
pub mod membership;
pub mod negotiate;
pub mod replica_sync;
pub mod replica_sync_coordinator;
pub mod s3_api;
pub(crate) mod subxt_client;
pub mod types;

pub use api::create_router;
pub use challenge_proofs::StorageProofSource;
pub use error::Error;
pub use negotiate::{AgreementTermsOf, NegotiateRequest, SignedTerms};
pub use provider_challenge::{
    self as challenge_responder, ChallengeChainClient, ChallengeError, ChallengeProofSource,
    ChallengeResponder, ChallengeResponderConfig, ChallengeResponderHandle,
    ChallengeResponseResult, DetectedChallenge, ResponderCommand,
};
/// The chain-state coordinator lives in the `provider-coordinator` crate; keep
/// the old module path working for existing consumers.
pub use provider_coordinator as chain_state_coordinator;
pub use provider_coordinator::{
    is_relevant_provider_event, refresh_if_relevant_event, refresh_provider_state, sync_constants,
    ChainState, ChainStateChainClient, ChainStateCoordinator, ChainStateCoordinatorHandle,
    NonceCounter, PalletConstants, ProviderLifecycleEvent,
};
pub use replica_sync::ReplicaSync;
pub use replica_sync_coordinator::{
    ReplicaSyncChainClient, ReplicaSyncCoordinator, ReplicaSyncCoordinatorConfig,
    ReplicaSyncCoordinatorHandle, SignedSyncRoots, SyncCommand, SyncCoordinatorStatus, SyncDuty,
    SyncResult,
};
pub use types::*;

use codec::Encode;
use provider_storage::{FsIndexManager, NonceStore, S3IndexManager, StorageBackend};
use sp_core::crypto::{ByteArray, Ss58Codec};
use sp_core::{ecdsa, ed25519, sr25519, Pair};
use sp_runtime::MultiSignature;
use std::sync::Arc;

/// Signature scheme of the provider's signing keypair — the key registered
/// on-chain as `public_key` and verified by the pallet via `MultiSignature`.
/// The extrinsic-submission account stays sr25519 regardless (see
/// [`ProviderState::with_seed_scheme`]). `Eth` is ecdsa over keccak digests
/// with revive-style account derivation — what Ethereum wallets produce.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum KeyScheme {
    #[default]
    Sr25519,
    Ed25519,
    Ecdsa,
    Eth,
}

/// The provider's signing keypair, scheme-tagged so every signature leaves
/// the node as a self-describing [`MultiSignature`].
#[allow(clippy::large_enum_variant)]
#[derive(Clone)]
pub enum ProviderKeypair {
    Sr25519(sr25519::Pair),
    Ed25519(ed25519::Pair),
    Ecdsa(ecdsa::Pair),
    Eth(ecdsa::KeccakPair),
}

impl ProviderKeypair {
    /// Derive from a SURI (e.g. `//Alice` or a mnemonic) for the given scheme.
    pub fn from_seed(seed: &str, scheme: KeyScheme) -> Result<Self, String> {
        fn derive<P: Pair>(seed: &str) -> Result<P, String> {
            P::from_string(seed, None).map_err(|e| format!("Failed to create keypair: {e:?}"))
        }
        Ok(match scheme {
            KeyScheme::Sr25519 => Self::Sr25519(derive(seed)?),
            KeyScheme::Ed25519 => Self::Ed25519(derive(seed)?),
            KeyScheme::Ecdsa => Self::Ecdsa(derive(seed)?),
            KeyScheme::Eth => Self::Eth(derive(seed)?),
        })
    }

    /// Sign a raw message, tagging the signature with its scheme.
    pub fn sign(&self, message: &[u8]) -> MultiSignature {
        match self {
            Self::Sr25519(pair) => MultiSignature::Sr25519(pair.sign(message)),
            Self::Ed25519(pair) => MultiSignature::Ed25519(pair.sign(message)),
            Self::Ecdsa(pair) => MultiSignature::Ecdsa(pair.sign(message)),
            Self::Eth(pair) => MultiSignature::Eth(pair.sign(message)),
        }
    }

    /// Raw public key bytes as registered on-chain: 32 for Sr25519/Ed25519,
    /// 33 (compressed) for Ecdsa/Eth.
    pub fn public_key_bytes(&self) -> Vec<u8> {
        match self {
            Self::Sr25519(pair) => pair.public().to_raw_vec(),
            Self::Ed25519(pair) => pair.public().to_raw_vec(),
            Self::Ecdsa(pair) => pair.public().to_raw_vec(),
            Self::Eth(pair) => pair.public().to_raw_vec(),
        }
    }

    /// Sign negotiated terms, bundling terms + scheme-tagged signature.
    pub fn sign_terms(&self, terms: AgreementTermsOf) -> SignedTerms {
        match self {
            Self::Sr25519(pair) => provider_negotiation::sign_terms(pair, terms),
            Self::Ed25519(pair) => provider_negotiation::sign_terms(pair, terms),
            Self::Ecdsa(pair) => provider_negotiation::sign_terms(pair, terms),
            Self::Eth(pair) => {
                // Upstream has no `From<KeccakSignature> for MultiSignature`,
                // so the generic helper can't cover Eth — same payload,
                // explicit wrap.
                let hash = sp_crypto_hashing::blake2_256(&terms.signing_payload());
                let signature = MultiSignature::Eth(pair.sign(&hash));
                SignedTerms { terms, signature }
            }
        }
    }
}

/// Everything a servable [`ProviderState`] requires.
pub struct ProviderDeps {
    /// Local storage backend.
    pub storage: Arc<dyn StorageBackend>,
    /// Persistence backing for the nonce counter.
    pub nonce_store: Arc<dyn NonceStore>,
    /// Verifies signed requests and enforces bucket roles.
    pub auth: Arc<provider_auth::Authenticator>,
}

/// Provider node state shared across handlers.
pub struct ProviderState {
    /// Local storage backend
    pub storage: Arc<dyn StorageBackend>,
    /// Provider account ID (SS58 encoded)
    pub provider_id: String,
    /// Signing keypair (optional, for dev/testing)
    pub keypair: Option<ProviderKeypair>,
    /// S3-compatible object index
    pub s3_index: S3IndexManager,
    /// File system drive index
    pub fs_index: FsIndexManager,
    /// Verifies signed requests and enforces bucket roles.
    pub auth: Arc<provider_auth::Authenticator>,
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
    fn from_parts(
        deps: ProviderDeps,
        provider_id: String,
        keypair: Option<ProviderKeypair>,
    ) -> Self {
        let ProviderDeps {
            storage,
            nonce_store,
            auth,
        } = deps;
        Self {
            storage,
            provider_id,
            keypair,
            s3_index: S3IndexManager::new(),
            fs_index: FsIndexManager::new(),
            auth,
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

    /// Create with a seed phrase or derivation path (e.g., "//Alice", "//Bob"),
    /// signing with the default sr25519 scheme.
    pub fn with_seed(deps: ProviderDeps, seed: &str) -> Result<Self, String> {
        Self::with_seed_scheme(deps, seed, KeyScheme::Sr25519)
    }

    /// Create with a seed phrase or derivation path and an explicit signing
    /// scheme.
    ///
    /// `provider_id` is always the sr25519 account derived from the seed —
    /// the extrinsic-submission account the provider is keyed by on-chain.
    /// The scheme only selects the signing keypair (the on-chain
    /// `public_key`), so a non-sr25519 provider keeps the same identity it
    /// registered with.
    pub fn with_seed_scheme(
        deps: ProviderDeps,
        seed: &str,
        scheme: KeyScheme,
    ) -> Result<Self, String> {
        let account = sr25519::Pair::from_string(seed, None)
            .map_err(|e| format!("Failed to create keypair: {e:?}"))?;
        let provider_id = account.public().to_ss58check();
        let keypair = ProviderKeypair::from_seed(seed, scheme)?;

        Ok(Self::from_parts(deps, provider_id, Some(keypair)))
    }

    /// Restrict the browser origins allowed via CORS. `None` (the default) keeps
    /// the permissive policy; `Some(list)` restricts to exactly those origins.
    pub fn with_cors_origins(mut self, origins: Option<Vec<String>>) -> Self {
        self.cors_allowed_origins = origins;
        self
    }

    /// Sign a message and return the SCALE-encoded [`MultiSignature`] as
    /// `0x`-prefixed hex — the same wire format `/negotiate` uses, so the
    /// scheme tag travels with every signature.
    ///
    /// Returns `Err(Error::SigningUnavailable)` if no keypair is configured.
    /// Callers must propagate this so the HTTP layer returns 503 rather than
    /// silently emitting a zeroed placeholder signature, which would be a
    /// cryptographically invalid commitment masquerading as a real one.
    pub fn sign(&self, message: &[u8]) -> Result<String, Error> {
        let keypair = self.keypair.as_ref().ok_or(Error::SigningUnavailable)?;
        Ok(format!("0x{}", hex::encode(keypair.sign(message).encode())))
    }

    /// Proof source for the challenge responder, backed by this state's
    /// storage.
    pub fn challenge_proof_source(&self) -> Arc<dyn ChallengeProofSource> {
        Arc::new(StorageProofSource::new(self.storage.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use provider_auth::Authenticator;
    use provider_storage::temp_rocksdb;

    /// Deps over a throwaway backend. Keep the returned guard bound for as
    /// long as the state is used.
    fn test_deps() -> (ProviderDeps, tempfile::TempDir) {
        let (storage, nonce_store, dir) = temp_rocksdb();
        let deps = ProviderDeps {
            storage,
            nonce_store,
            auth: Arc::new(Authenticator::new(provider_auth::StaticMembershipResolver(
                vec![],
            ))),
        };
        (deps, dir)
    }

    #[test]
    fn sign_without_keypair_refuses_with_signing_unavailable() {
        // The pre-fix behaviour silently returned 64 zero bytes. The new
        // contract is that `sign()` MUST return `Err(SigningUnavailable)`
        // when no keypair is configured, so the HTTP layer can map it to a
        // 503 instead of emitting a cryptographically invalid placeholder.
        let (deps, _dir) = test_deps();
        let state = ProviderState::with_provider_id(deps, "no-key-provider".to_string());
        let err = state
            .sign(b"any message")
            .expect_err("must refuse to sign without a keypair");
        assert!(matches!(err, Error::SigningUnavailable));
    }

    #[test]
    fn sign_with_keypair_returns_real_sr25519_signature() {
        // Round-trip: sign with //Alice, decode the SCALE-encoded
        // MultiSignature, verify against Alice's public key. This catches
        // any regression where sign() ever returns a placeholder again, and
        // also catches the more subtle case where the bytes look random but
        // aren't valid sr25519.
        let (deps, _dir) = test_deps();
        let state = ProviderState::with_seed(deps, "//Alice").unwrap();
        let message = b"commitment-payload-bytes";

        let sig_hex = state.sign(message).expect("signing succeeds with keypair");
        let sig_bytes = hex::decode(sig_hex.strip_prefix("0x").unwrap()).unwrap();
        assert_eq!(
            sig_bytes.len(),
            65,
            "SCALE MultiSignature: 1 variant byte + 64 sig bytes"
        );

        let sig = sig_from_hex(&sig_hex);
        assert_ne!(sig.0, [0u8; 64], "must not return a zeroed placeholder");
        let alice = keypair_for("//Alice");
        assert!(
            sr25519::Pair::verify(&sig, message, &alice.public()),
            "signature did not verify under //Alice's public key"
        );
    }

    /// Decode an `0x`-prefixed SCALE `MultiSignature` hex into the inner
    /// `sr25519::Signature`, asserting the variant tag.
    fn sig_from_hex(sig_hex: &str) -> sr25519::Signature {
        use codec::Decode;
        let bytes = hex::decode(sig_hex.strip_prefix("0x").unwrap()).unwrap();
        match MultiSignature::decode(&mut &bytes[..]).expect("valid SCALE MultiSignature") {
            MultiSignature::Sr25519(sig) => sig,
            other => panic!("expected an Sr25519 signature, got {other:?}"),
        }
    }

    /// sign_terms produces, for every scheme, a signature the pallet's
    /// terms verification accepts: over `blake2_256(signing_payload())`,
    /// against the account derived from the raw registered key.
    #[test]
    fn sign_terms_round_trips_for_every_scheme() {
        use sp_runtime::traits::{IdentifyAccount, Verify};
        use sp_runtime::{AccountId32, MultiSigner};

        let terms = AgreementTermsOf {
            owner: AccountId32::new([7u8; 32]),
            max_bytes: 1024,
            duration: 50,
            price_per_byte: 5,
            valid_until: 100,
            nonce: 1,
            bucket_id: None,
            replica_params: None,
        };

        for scheme in [
            KeyScheme::Sr25519,
            KeyScheme::Ed25519,
            KeyScheme::Ecdsa,
            KeyScheme::Eth,
        ] {
            let keypair = ProviderKeypair::from_seed("//Alice", scheme).unwrap();
            let key = keypair.public_key_bytes();
            let signed = keypair.sign_terms(terms.clone());
            assert_eq!(
                signed.terms, terms,
                "{scheme:?} must bundle the terms unchanged"
            );

            let hash = sp_crypto_hashing::blake2_256(&signed.terms.signing_payload());
            let signer = match &signed.signature {
                MultiSignature::Sr25519(_) => {
                    MultiSigner::Sr25519(sr25519::Public::try_from(key.as_slice()).unwrap())
                }
                MultiSignature::Ed25519(_) => {
                    MultiSigner::Ed25519(ed25519::Public::try_from(key.as_slice()).unwrap())
                }
                MultiSignature::Ecdsa(_) => {
                    MultiSigner::Ecdsa(ecdsa::Public::try_from(key.as_slice()).unwrap())
                }
                MultiSignature::Eth(_) => {
                    MultiSigner::Eth(ecdsa::KeccakPublic::try_from(key.as_slice()).unwrap())
                }
            };
            assert!(
                signed.signature.verify(&hash[..], &signer.into_account()),
                "{scheme:?} terms signature failed verification"
            );
        }
    }

    /// Every scheme round-trips: sign() emits a SCALE MultiSignature whose
    /// variant matches the configured scheme and whose registered key shape
    /// is 32 (Sr25519/Ed25519) or 33 (Ecdsa/Eth) bytes.
    #[test]
    fn sign_round_trips_for_every_scheme() {
        use codec::Decode;
        use sp_runtime::traits::{IdentifyAccount, Verify};
        use sp_runtime::MultiSigner;

        let msg = b"scheme-round-trip";
        for (scheme, key_len) in [
            (KeyScheme::Sr25519, 32),
            (KeyScheme::Ed25519, 32),
            (KeyScheme::Ecdsa, 33),
            (KeyScheme::Eth, 33),
        ] {
            let keypair = ProviderKeypair::from_seed("//Alice", scheme).unwrap();
            let key = keypair.public_key_bytes();
            assert_eq!(key.len(), key_len, "{scheme:?} key length");

            let encoded = keypair.sign(msg).encode();
            let sig = MultiSignature::decode(&mut &encoded[..]).unwrap();
            let matches_scheme = matches!(
                (&sig, scheme),
                (MultiSignature::Sr25519(_), KeyScheme::Sr25519)
                    | (MultiSignature::Ed25519(_), KeyScheme::Ed25519)
                    | (MultiSignature::Ecdsa(_), KeyScheme::Ecdsa)
                    | (MultiSignature::Eth(_), KeyScheme::Eth)
            );
            assert!(matches_scheme, "{scheme:?} produced {sig:?}");

            // Verify the same way the pallet does: derive the expected
            // account from the raw key bytes for this scheme.
            let signer = match &sig {
                MultiSignature::Sr25519(_) => {
                    MultiSigner::Sr25519(sr25519::Public::try_from(key.as_slice()).unwrap())
                }
                MultiSignature::Ed25519(_) => {
                    MultiSigner::Ed25519(ed25519::Public::try_from(key.as_slice()).unwrap())
                }
                MultiSignature::Ecdsa(_) => {
                    MultiSigner::Ecdsa(ecdsa::Public::try_from(key.as_slice()).unwrap())
                }
                MultiSignature::Eth(_) => {
                    MultiSigner::Eth(ecdsa::KeccakPublic::try_from(key.as_slice()).unwrap())
                }
            };
            assert!(
                sig.verify(&msg[..], &signer.into_account()),
                "{scheme:?} signature failed verification"
            );
        }
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
        let (deps, _dir) = test_deps();
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
        let (alice_deps, _alice_dir) = test_deps();
        let (bob_deps, _bob_dir) = test_deps();
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
        let (deps, _dir) = test_deps();
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
