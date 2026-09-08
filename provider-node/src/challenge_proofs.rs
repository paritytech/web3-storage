// SPDX-License-Identifier: GPL-3.0-only

//! Adapts [`StorageBackend`] to [`ChallengeProofSource`], so the challenge
//! responder can be given proof access without depending on the rest of
//! [`ProviderState`](crate::ProviderState).

use provider_challenge::{ChallengeError, ChallengeProofSource};
use provider_storage::StorageBackend;
use std::sync::Arc;

/// [`ChallengeProofSource`] backed directly by a storage backend.
pub struct StorageProofSource(Arc<dyn StorageBackend>);

impl StorageProofSource {
    pub fn new(storage: Arc<dyn StorageBackend>) -> Self {
        Self(storage)
    }
}

impl ChallengeProofSource for StorageProofSource {
    fn get_mmr_proof_for_commitment(
        &self,
        bucket_id: storage_primitives::BucketId,
        mmr_root: sp_core::H256,
        start_seq: u64,
        leaf_index: u64,
    ) -> Result<storage_primitives::MmrProof, ChallengeError> {
        self.0
            .get_mmr_proof_for_commitment(bucket_id, mmr_root, start_seq, leaf_index)
            .map_err(|e| ChallengeError::Storage(e.to_string()))
    }

    fn get_chunk_at_index(
        &self,
        data_root: sp_core::H256,
        chunk_index: u64,
    ) -> Result<(Vec<u8>, storage_primitives::MerkleProof), ChallengeError> {
        self.0
            .get_chunk_at_index(data_root, chunk_index)
            .map_err(|e| ChallengeError::Storage(e.to_string()))
    }

    fn deletion_receipt_covering(
        &self,
        bucket_id: storage_primitives::BucketId,
        seq: u64,
    ) -> Option<provider_challenge::DeletionReceipt> {
        self.0.deletion_receipt_covering(bucket_id, seq).map(|r| {
            provider_challenge::DeletionReceipt {
                mmr_root: r.mmr_root,
                new_start_seq: r.new_start_seq,
                admin: r.admin,
                signature: r.signature,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use provider_storage::temp_rocksdb;
    use storage_primitives::blake2_256;

    /// Constructing this from a bare `Arc<dyn StorageBackend>`, with no
    /// `ProviderState` in sight, is the point of the newtype.
    fn source_over_committed_chunk() -> (Arc<dyn ChallengeProofSource>, sp_core::H256, Vec<u8>) {
        let (storage, _nonce_store, _dir) = temp_rocksdb();
        storage.init_bucket(1, 1024 * 1024).unwrap();

        let chunk_data = b"proof-source-test-chunk".to_vec();
        let chunk_hash = blake2_256(&chunk_data);
        storage
            .store_node(1, chunk_hash, chunk_data.clone(), None)
            .unwrap();
        storage.commit(1, vec![chunk_hash]).unwrap();

        let source: Arc<dyn ChallengeProofSource> =
            Arc::new(StorageProofSource::new(Arc::clone(&storage)));
        (source, chunk_hash, chunk_data)
    }

    #[test]
    fn get_mmr_proof_for_commitment_matches_the_backend() {
        let (storage, _nonce_store, _dir) = temp_rocksdb();
        storage.init_bucket(1, 1024 * 1024).unwrap();
        let chunk_hash = blake2_256(b"mmr-proof-test-chunk");
        storage
            .store_node(1, chunk_hash, b"mmr-proof-test-chunk".to_vec(), None)
            .unwrap();
        storage.commit(1, vec![chunk_hash]).unwrap();
        let info = storage.get_bucket(1).unwrap();

        let expected = storage
            .get_mmr_proof_for_commitment(1, info.mmr_root, info.start_seq, 0)
            .unwrap();
        let source = StorageProofSource::new(Arc::clone(&storage));

        assert_eq!(
            source
                .get_mmr_proof_for_commitment(1, info.mmr_root, info.start_seq, 0)
                .unwrap(),
            expected
        );
    }

    #[test]
    fn get_chunk_at_index_matches_the_backend() {
        let (source, chunk_hash, chunk_data) = source_over_committed_chunk();

        let (data, proof) = source.get_chunk_at_index(chunk_hash, 0).unwrap();
        assert_eq!(data, chunk_data);
        assert!(proof.siblings.is_empty());
        assert!(proof.path.is_empty());
    }

    #[test]
    fn backend_error_surfaces_as_challenge_error_storage() {
        let (storage, _nonce_store, _dir) = temp_rocksdb();
        storage.init_bucket(1, 1024 * 1024).unwrap();

        let backend_err = storage
            .get_chunk_at_index(sp_core::H256::zero(), 0)
            .unwrap_err();

        let source = StorageProofSource::new(storage);
        let err = source
            .get_chunk_at_index(sp_core::H256::zero(), 0)
            .unwrap_err();

        match err {
            ChallengeError::Storage(msg) => assert_eq!(msg, backend_err.to_string()),
            other => panic!("expected ChallengeError::Storage, got {other:?}"),
        }
    }
}
