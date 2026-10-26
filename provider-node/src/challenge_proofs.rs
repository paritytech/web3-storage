// SPDX-License-Identifier: GPL-3.0-only

//! Adapts [`StorageBackend`] to [`ChallengeProofSource`], so the challenge
//! responder can be given proof access without depending on the rest of
//! [`ProviderState`](crate::ProviderState).

use provider_challenge::{ChallengeError, ChallengeProofSource, ProofTarget};
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
        commitment: &storage_primitives::Commitment,
        leaf_index: u64,
    ) -> Result<storage_primitives::MmrProof, ChallengeError> {
        let target = ProofTarget::MmrLeaf {
            bucket_id,
            leaf_index,
        };
        self.0
            .get_mmr_proof_for_commitment(bucket_id, commitment, leaf_index)
            .map_err(|e| classify(e, target))
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
            .map_err(|e| classify(e, target))
    }
}

/// Translate a storage-layer failure into what the challenge responder needs
/// to know: is the proof data provably gone, or did the backend itself fail?
///
/// `NodeNotFound`, `BucketNotFound`, `ChunkIndexOutOfRange` and `LeafNotFound` mean
/// the storage layer looked and found nothing there - genuine absence, once
/// the reads underneath stopped collapsing a backend failure into the same
/// signal (see `StorageBackend`). Everything else means the
/// lookup didn't complete cleanly, so the data's actual presence is unknown.
/// `RocksDb`, `ColumnFamilyMissing` and `Serialization` are the ones a read
/// path really produces: the engine failed, the database layout is wrong, or
/// a record decoded to garbage - in none of those did storage get to look and
/// find nothing. It also includes variants that structurally
/// cannot come back from either read path (`ChildrenMissing`, `QuotaExceeded`
/// and `InvalidHash` are write-side only; `RootNotFound` is produced solely
/// by `commit`, confirmed by grepping the backend for its only call site) -
/// treating an impossible value as "the backend is behaving unexpectedly" is
/// the safe direction, since the alternative would treat it as provable data
/// loss on no real evidence.
fn classify(e: provider_storage::Error, target: ProofTarget) -> ChallengeError {
    use provider_storage::Error;
    match e {
        Error::NodeNotFound(_)
        | Error::BucketNotFound(_)
        | Error::ChunkIndexOutOfRange { .. }
        | Error::LeafNotFound { .. } => ChallengeError::ProofDataMissing { target },
        other => ChallengeError::StorageUnavailable {
            target,
            detail: other.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use provider_storage::{temp_rocksdb, ChunkTreeNode};
    use storage_primitives::hash_leaf;

    /// A commitment for tests that fail before the commitment is read.
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
        assert!(!err.is_retryable());
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

    /// A backend that fails outright (not "empty", genuinely broken) must
    /// not be reported the same way as absence - this is the
    /// `StorageUnavailable` row of the mapping. There is no way to break a
    /// real `DiskStorage` from outside its own crate, so this uses a bare
    /// in-test `StorageBackend` that always errors, proving the boundary
    /// classifies on the error returned rather than on any RocksDB detail.
    struct AlwaysFailingBackend;

    impl provider_storage::StorageBackend for AlwaysFailingBackend {
        fn init_bucket(
            &self,
            _: storage_primitives::BucketId,
            _: u64,
        ) -> Result<(), provider_storage::Error> {
            unimplemented!("not exercised by this test")
        }
        fn get_bucket(
            &self,
            _: storage_primitives::BucketId,
        ) -> Result<Option<provider_storage::BucketInfo>, provider_storage::Error> {
            unimplemented!("not exercised by this test")
        }
        fn bucket_ids(&self) -> Vec<storage_primitives::BucketId> {
            unimplemented!("not exercised by this test")
        }
        fn total_nodes(&self) -> u64 {
            unimplemented!("not exercised by this test")
        }
        fn total_bytes(&self) -> u64 {
            unimplemented!("not exercised by this test")
        }
        fn store_node(
            &self,
            _: storage_primitives::BucketId,
            _: sp_core::H256,
            _: ChunkTreeNode,
        ) -> Result<(), provider_storage::Error> {
            unimplemented!("not exercised by this test")
        }
        fn get_node(
            &self,
            _: &sp_core::H256,
        ) -> Result<Option<ChunkTreeNode>, provider_storage::Error> {
            unimplemented!("not exercised by this test")
        }
        fn check_exists(
            &self,
            _: storage_primitives::BucketId,
            hashes: &[sp_core::H256],
        ) -> (Vec<sp_core::H256>, Vec<sp_core::H256>) {
            (vec![], hashes.to_vec())
        }
        fn commit(
            &self,
            _: storage_primitives::BucketId,
            _: Vec<sp_core::H256>,
        ) -> Result<provider_storage::CommitOutcome, provider_storage::Error> {
            unimplemented!("not exercised by this test")
        }
        fn delete_before(
            &self,
            _: storage_primitives::BucketId,
            _: u64,
        ) -> Result<(sp_core::H256, u64, u64), provider_storage::Error> {
            unimplemented!("not exercised by this test")
        }
        fn get_mmr_proof(
            &self,
            _: storage_primitives::BucketId,
            _: u64,
        ) -> Result<storage_primitives::MmrProof, provider_storage::Error> {
            unimplemented!("not exercised by this test")
        }
        fn get_mmr_proof_for_commitment(
            &self,
            _: storage_primitives::BucketId,
            _: &storage_primitives::Commitment,
            _: u64,
        ) -> Result<storage_primitives::MmrProof, provider_storage::Error> {
            Err(provider_storage::Error::ColumnFamilyMissing("nodes"))
        }
        fn get_chunk_at_index(
            &self,
            _: sp_core::H256,
            _: u64,
        ) -> Result<(Vec<u8>, Vec<sp_core::H256>), provider_storage::Error> {
            Err(provider_storage::Error::Serialization(
                "simulated corrupt record".to_string(),
            ))
        }
        fn get_mmr_peaks(
            &self,
            _: storage_primitives::BucketId,
        ) -> Result<(sp_core::H256, Vec<sp_core::H256>), provider_storage::Error> {
            unimplemented!("not exercised by this test")
        }
    }

    #[test]
    fn mmr_proof_backend_failure_is_reported_as_storage_unavailable_not_missing() {
        let source = StorageProofSource::new(Arc::new(AlwaysFailingBackend));

        let err = source
            .get_mmr_proof_for_commitment(1, &any_commitment(), 0)
            .unwrap_err();

        assert!(err.is_retryable());
        assert!(!err.risks_slashing());
        match err {
            ChallengeError::StorageUnavailable { target, detail } => {
                assert_eq!(
                    target,
                    ProofTarget::MmrLeaf {
                        bucket_id: 1,
                        leaf_index: 0,
                    }
                );
                assert_eq!(detail, "Column family not found: nodes".to_string());
            }
            other => panic!("expected StorageUnavailable, got {other:?}"),
        }
    }

    #[test]
    fn chunk_backend_failure_is_reported_as_storage_unavailable_not_missing() {
        let source = StorageProofSource::new(Arc::new(AlwaysFailingBackend));
        let root = sp_core::H256::repeat_byte(7);

        let err = source.get_chunk_at_index(root, 3).unwrap_err();

        assert!(err.is_retryable());
        assert!(!err.risks_slashing());
        match err {
            ChallengeError::StorageUnavailable { target, detail } => {
                assert_eq!(
                    target,
                    ProofTarget::Chunk {
                        data_root: root,
                        chunk_index: 3,
                    }
                );
                assert_eq!(
                    detail,
                    "Serialization error: simulated corrupt record".to_string()
                );
            }
            other => panic!("expected StorageUnavailable, got {other:?}"),
        }
    }
}
