// SPDX-License-Identifier: Apache-2.0

//! [`BlobStore`] over the provider's HTTP API, with `storage-client`.

use crate::format::Cid;
use crate::tree::{BlobLength, BlobStore};
use crate::Result;
use storage_client::{ChunkingStrategy, StorageUserClient};
use storage_primitives::{BucketId, Commitment, MmrProof};

/// The provider's blob routes over HTTP. The client must not have encryption
/// enabled: the stored bytes would then not match the CIDs the tree computes.
#[async_trait::async_trait]
impl BlobStore for StorageUserClient {
    async fn put_blob(&self, bucket_id: BucketId, data: &[u8]) -> Result<Cid> {
        Ok(self
            .upload(bucket_id, data, ChunkingStrategy::default())
            .await?)
    }

    /// With [`BlobLength::AtMost`], the read requests one byte more than
    /// the maximum.
    async fn get_blob(&self, bucket_id: BucketId, cid: Cid, length: BlobLength) -> Result<Vec<u8>> {
        let max = match length {
            BlobLength::Exact(size) => size,
            BlobLength::AtMost(max) => max.saturating_add(1),
        };
        Ok(self.download(bucket_id, &cid, 0, max).await?)
    }

    async fn commit(&self, bucket_id: BucketId, data_roots: Vec<Cid>) -> Result<()> {
        StorageUserClient::commit(self, bucket_id, data_roots).await?;
        Ok(())
    }

    async fn commitment(&self, bucket_id: BucketId) -> Result<Option<Commitment>> {
        let commitment = self.get_commitment_if_exists(bucket_id).await?;
        Ok(commitment.map(|c| Commitment {
            mmr_root: c.mmr_root,
            start_seq: c.start_seq,
            leaf_count: c.leaf_count,
        }))
    }

    async fn mmr_proof(&self, bucket_id: BucketId, leaf_index: u64) -> Result<MmrProof> {
        Ok(self.get_mmr_proof(bucket_id, leaf_index).await?)
    }
}
