// SPDX-License-Identifier: GPL-3.0-only

//! Adapts [`StorageBackend`] to [`ChallengeProofSource`], so the challenge
//! responder can be given proof access without depending on the rest of
//! [`ProviderState`](crate::ProviderState).

use provider_challenge::{ChallengeError, ChallengeProofSource, ProofTarget};
use provider_storage::{Error, StorageBackend};
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
        commitment: &storage_primitives::Commitment,
        leaf_index: u64,
    ) -> Result<storage_primitives::MmrProof, ChallengeError> {
        let target = ProofTarget::MmrLeaf {
            bucket_id,
            leaf_index,
        };
        self.0
            .get_mmr_proof_for_commitment(bucket_id, commitment, leaf_index)
            .map_err(|e| match e {
                Error::BucketNotFound(_)
                | Error::LeafNotFound { .. }
                | Error::LeavesNotHeld { .. } => ChallengeError::ProofDataMissing { target },
                e => ChallengeError::Storage {
                    reason: e.to_string(),
                },
            })
    }

    fn get_chunk_at_index(
        &self,
        data_root: sp_core::H256,
        chunk_index: u64,
    ) -> Result<(Vec<u8>, Vec<sp_core::H256>), ChallengeError> {
        let target = ProofTarget::Chunk {
            data_root,
            chunk_index,
        };
        self.0
            .get_chunk_at_index(data_root, chunk_index)
            .map_err(|e| match e {
                Error::NodeNotFound(_) | Error::ChunkIndexOutOfRange { .. } => {
                    ChallengeError::ProofDataMissing { target }
                }
                e => ChallengeError::Storage {
                    reason: e.to_string(),
                },
            })
    }

    fn deletion_receipt_covering(
        &self,
        bucket_id: storage_primitives::BucketId,
        seq: u64,
    ) -> Result<Option<provider_challenge::DeletionReceipt>, ChallengeError> {
        let receipt = self
            .0
            .deletion_receipt_covering(bucket_id, seq)
            .map_err(|e| ChallengeError::Storage {
                reason: e.to_string(),
            })?;
        Ok(receipt.map(|r| provider_challenge::DeletionReceipt {
            mmr_root: r.mmr_root,
            new_start_seq: r.new_start_seq,
            admin: r.admin,
            signature: r.signature,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use provider_storage::{temp_rocksdb, ChunkTreeNode};
    use storage_primitives::hash_leaf;

    /// A one-leaf commitment, for tests that never reach a real proof.
    fn any_commitment() -> storage_primitives::Commitment {
        storage_primitives::Commitment {
            mmr_root: sp_core::H256::zero(),
            start_seq: 0,
            leaf_count: 1,
        }
    }

    /// Constructing this from a bare `Arc<dyn StorageBackend>`, with no
    /// `ProviderState` in sight, is the point of the newtype.
    fn source_over_committed_chunk() -> (Arc<dyn ChallengeProofSource>, sp_core::H256, Vec<u8>) {
        let (storage, _dir) = temp_rocksdb();
        storage.init_bucket(1, 1024 * 1024).unwrap();

        let chunk_data = b"proof-source-test-chunk".to_vec();
        let chunk_hash = hash_leaf(&chunk_data);
        storage
            .store_node(1, chunk_hash, ChunkTreeNode::Chunk(chunk_data.clone()))
            .unwrap();
        storage.commit(1, vec![chunk_hash]).unwrap();

        let source: Arc<dyn ChallengeProofSource> =
            Arc::new(StorageProofSource::new(Arc::clone(&storage)));
        (source, chunk_hash, chunk_data)
    }

    #[test]
    fn get_mmr_proof_matches_the_backend() {
        let (storage, _dir) = temp_rocksdb();
        storage.init_bucket(1, 1024 * 1024).unwrap();
        let chunk_hash = hash_leaf(b"mmr-proof-test-chunk");
        storage
            .store_node(
                1,
                chunk_hash,
                ChunkTreeNode::Chunk(b"mmr-proof-test-chunk".to_vec()),
            )
            .unwrap();
        storage.commit(1, vec![chunk_hash]).unwrap();

        let commitment = {
            let info = storage.get_bucket(1).unwrap().unwrap();
            storage_primitives::Commitment {
                mmr_root: info.mmr_root,
                start_seq: info.start_seq,
                leaf_count: info.leaf_count,
            }
        };
        let expected = storage
            .get_mmr_proof_for_commitment(1, &commitment, 0)
            .unwrap();
        let source = StorageProofSource::new(Arc::clone(&storage));

        assert_eq!(
            source
                .get_mmr_proof_for_commitment(1, &commitment, 0)
                .unwrap(),
            expected
        );
    }

    #[test]
    fn get_chunk_at_index_matches_the_backend() {
        let (source, chunk_hash, chunk_data) = source_over_committed_chunk();

        let (data, proof) = source.get_chunk_at_index(chunk_hash, 0).unwrap();
        assert_eq!(data, chunk_data);
        assert!(proof.is_empty());
    }

    /// A data root the backend has never seen (empty tree, nothing
    /// committed) is genuine absence, not a backend failure - this is the
    /// `ProofDataMissing` row of the mapping.
    #[test]
    fn missing_chunk_data_is_reported_as_provably_missing() {
        let (storage, _dir) = temp_rocksdb();
        storage.init_bucket(1, 1024 * 1024).unwrap();

        let source = StorageProofSource::new(storage);
        let err = source
            .get_chunk_at_index(sp_core::H256::zero(), 0)
            .unwrap_err();

        assert!(err.risks_slashing());
        match err {
            ChallengeError::ProofDataMissing { target } => assert_eq!(
                target,
                ProofTarget::Chunk {
                    data_root: sp_core::H256::zero(),
                    chunk_index: 0,
                }
            ),
            other => panic!("expected ProofDataMissing, got {other:?}"),
        }
    }

    /// A bucket that was never created is the same genuine-absence signal,
    /// on the MMR-proof side of the mapping.
    #[test]
    fn missing_bucket_is_reported_as_provably_missing() {
        let (storage, _dir) = temp_rocksdb();
        let source = StorageProofSource::new(storage);

        let err = source
            .get_mmr_proof_for_commitment(999, &any_commitment(), 0)
            .unwrap_err();

        assert!(err.risks_slashing());
        match err {
            ChallengeError::ProofDataMissing { target } => assert_eq!(
                target,
                ProofTarget::MmrLeaf {
                    bucket_id: 999,
                    leaf_index: 0,
                }
            ),
            other => panic!("expected ProofDataMissing, got {other:?}"),
        }
    }

    /// A commitment citing leaves this store does not hold (pruned, or not
    /// yet synced) can never be answered: provably missing, not retryable.
    #[test]
    fn leaves_not_held_are_reported_as_provably_missing() {
        let (storage, _dir) = temp_rocksdb();
        storage.init_bucket(1, 1024 * 1024).unwrap();
        let source = StorageProofSource::new(storage);

        let err = source
            .get_mmr_proof_for_commitment(1, &any_commitment(), 0)
            .unwrap_err();

        assert!(
            matches!(err, ChallengeError::ProofDataMissing { .. }),
            "expected ProofDataMissing, got {err:?}"
        );
    }

    /// A failure that is not absence - here, a leaf index outside the cited
    /// commitment - is a storage failure, not missing proof data.
    #[test]
    fn other_storage_errors_are_not_reported_as_missing_data() {
        let (storage, _dir) = temp_rocksdb();
        storage.init_bucket(1, 1024 * 1024).unwrap();
        let source = StorageProofSource::new(storage);

        let err = source
            .get_mmr_proof_for_commitment(1, &any_commitment(), 5)
            .unwrap_err();

        assert!(!err.risks_slashing());
        assert!(
            matches!(err, ChallengeError::Storage { .. }),
            "expected Storage, got {err:?}"
        );
    }
}
