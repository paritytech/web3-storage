//! Subxt-based production chain client for the challenge responder.

use crate::challenge_responder::ChallengeChainClient;
use crate::challenge_responder::DetectedChallenge;
use crate::Error;
use sp_core::H256;
use subxt::dynamic::Value;
use subxt::ext::scale_value::value;

/// Production implementation that talks to the chain via subxt.
/// Not yet wired into command.rs — scaffolding for when poll_challenges is implemented.
#[allow(dead_code)]
pub struct SubxtChallengeChainClient {
    api: subxt::OnlineClient<subxt::PolkadotConfig>,
    signer: subxt_signer::sr25519::Keypair,
}

#[allow(dead_code)]
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

#[async_trait::async_trait]
impl ChallengeChainClient for SubxtChallengeChainClient {
    async fn poll_challenges(&self) -> Result<Vec<DetectedChallenge>, Error> {
        let _storage = self
            .api
            .storage()
            .at_latest()
            .await
            .map_err(|e| Error::Internal(format!("Failed to get storage: {e}")))?;

        // TODO: Implement proper storage query for Challenges
        // For now, return empty - challenges would be detected via events
        Ok(vec![])
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
    }
}
