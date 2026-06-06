//! Subxt-based production chain client for the challenge responder.

use crate::challenge_responder::ChallengeChainClient;
use crate::challenge_responder::DetectedChallenge;
use crate::Error;
use sp_core::H256;
use std::future::Future;
use std::pin::Pin;

/// Production implementation that talks to the chain via subxt.
pub struct SubxtChallengeChainClient {
    api: subxt::OnlineClient<subxt::PolkadotConfig>,
    signer: subxt_signer::sr25519::Keypair,
}

impl SubxtChallengeChainClient {
    /// Connect to the chain and create a signer from the seed URI.
    pub async fn connect(chain_ws_url: &str, seed: &str) -> Result<Self, Error> {
        let api = subxt::OnlineClient::<subxt::PolkadotConfig>::from_url(chain_ws_url)
            .await
            .map_err(|e| Error::Internal(format!("Failed to connect to chain: {e}")))?;

        let uri: subxt_signer::SecretUri = seed
            .parse()
            .map_err(|e| Error::Internal(format!("Invalid seed URI: {e}")))?;
        let signer = subxt_signer::sr25519::Keypair::from_uri(&uri)
            .map_err(|e| Error::Internal(format!("Failed to create signer: {e}")))?;

        tracing::info!(
            "Challenge responder signer: {}",
            sp_core::crypto::AccountId32::from(signer.public_key().0).to_string()
        );
        tracing::info!("Challenge responder connected to {}", chain_ws_url);

        Ok(Self { api, signer })
    }
}

impl ChallengeChainClient for SubxtChallengeChainClient {
    fn poll_challenges(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<DetectedChallenge>, Error>> + Send + '_>> {
        Box::pin(async move {
            let _storage = self
                .api
                .storage()
                .at_latest()
                .await
                .map_err(|e| Error::Internal(format!("Failed to get storage: {e}")))?;

            // TODO: Implement proper storage query for Challenges
            // For now, return empty - challenges would be detected via events
            Ok(vec![])
        })
    }

    fn submit_response(
        &self,
        challenge_id: (u32, u16),
        chunk_data: Vec<u8>,
        mmr_proof: storage_primitives::MmrProof,
        chunk_proof: storage_primitives::MerkleProof,
    ) -> Pin<Box<dyn Future<Output = Result<H256, Error>> + Send + '_>> {
        Box::pin(async move {
            // Build ChallengeId
            let challenge_id_val = subxt::dynamic::Value::named_composite(vec![
                (
                    "deadline",
                    subxt::dynamic::Value::u128(challenge_id.0 as u128),
                ),
                ("index", subxt::dynamic::Value::u128(challenge_id.1 as u128)),
            ]);

            // Build MmrProof value
            let mmr_proof_val = subxt::dynamic::Value::named_composite(vec![
                (
                    "peaks",
                    subxt::dynamic::Value::unnamed_composite(
                        mmr_proof
                            .peaks
                            .iter()
                            .map(|p| subxt::dynamic::Value::from_bytes(p.as_bytes()))
                            .collect::<Vec<_>>(),
                    ),
                ),
                (
                    "leaf",
                    subxt::dynamic::Value::named_composite(vec![
                        (
                            "data_root",
                            subxt::dynamic::Value::from_bytes(mmr_proof.leaf.data_root.as_bytes()),
                        ),
                        (
                            "data_size",
                            subxt::dynamic::Value::u128(mmr_proof.leaf.data_size as u128),
                        ),
                        (
                            "total_size",
                            subxt::dynamic::Value::u128(mmr_proof.leaf.total_size as u128),
                        ),
                    ]),
                ),
                (
                    "leaf_proof",
                    subxt::dynamic::Value::named_composite(vec![
                        (
                            "siblings",
                            subxt::dynamic::Value::unnamed_composite(
                                mmr_proof
                                    .leaf_proof
                                    .siblings
                                    .iter()
                                    .map(|s| subxt::dynamic::Value::from_bytes(s.as_bytes()))
                                    .collect::<Vec<_>>(),
                            ),
                        ),
                        (
                            "path",
                            subxt::dynamic::Value::unnamed_composite(
                                mmr_proof
                                    .leaf_proof
                                    .path
                                    .iter()
                                    .map(|b| subxt::dynamic::Value::bool(*b))
                                    .collect::<Vec<_>>(),
                            ),
                        ),
                    ]),
                ),
            ]);

            // Build chunk proof value
            let chunk_proof_val = subxt::dynamic::Value::named_composite(vec![
                (
                    "siblings",
                    subxt::dynamic::Value::unnamed_composite(
                        chunk_proof
                            .siblings
                            .iter()
                            .map(|s| subxt::dynamic::Value::from_bytes(s.as_bytes()))
                            .collect::<Vec<_>>(),
                    ),
                ),
                (
                    "path",
                    subxt::dynamic::Value::unnamed_composite(
                        chunk_proof
                            .path
                            .iter()
                            .map(|b| subxt::dynamic::Value::bool(*b))
                            .collect::<Vec<_>>(),
                    ),
                ),
            ]);

            // Build ChallengeResponse::Proof variant
            let response_val = subxt::dynamic::Value::named_variant(
                "Proof",
                vec![
                    ("chunk_data", subxt::dynamic::Value::from_bytes(&chunk_data)),
                    ("mmr_proof", mmr_proof_val),
                    ("chunk_proof", chunk_proof_val),
                ],
            );

            let tx = subxt::dynamic::tx(
                "StorageProvider",
                "respond_to_challenge",
                vec![challenge_id_val, response_val],
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

            Ok(H256::zero())
        })
    }
}
