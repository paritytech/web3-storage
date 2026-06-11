//! S3 bucket metadata index.
//!
//! Provides a per-bucket sorted key→metadata map for S3-compatible object storage.
//! The index is stored in-memory (BTreeMap) with optional JSON persistence to disk.

use codec::Encode;
use dashmap::DashMap;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use sp_core::H256;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use storage_primitives::{blake2_256, hash_children, BucketId};

/// Per-object metadata stored in the index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectMeta {
    /// Merkle tree root of the chunked data
    pub data_root: H256,
    /// Original data size in bytes
    pub size: u64,
    /// MIME content type
    pub content_type: String,
    /// ETag (hex of data_root)
    pub etag: String,
    /// Last modified timestamp (Unix epoch seconds)
    pub last_modified: u64,
    /// User-defined metadata (x-amz-meta-* headers)
    pub user_metadata: Vec<(String, String)>,
    /// MMR leaf index from commit
    pub leaf_index: u64,
}

/// An object entry returned in list responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectEntry {
    pub key: String,
    pub size: u64,
    pub last_modified: u64,
    pub etag: String,
}

/// Result of a list operation with S3 semantics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListResult {
    pub contents: Vec<ObjectEntry>,
    pub common_prefixes: Vec<String>,
    pub is_truncated: bool,
    pub next_continuation_token: Option<String>,
    pub key_count: u32,
}

/// Per-bucket sorted key→metadata index.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BucketIndex {
    objects: BTreeMap<String, ObjectMeta>,
    pub object_count: u64,
    pub total_size: u64,
}

impl BucketIndex {
    /// Insert or update an object. Returns the old metadata if overwriting.
    pub fn put(&mut self, key: String, meta: ObjectMeta) -> Option<ObjectMeta> {
        let new_size = meta.size;
        let old = self.objects.insert(key, meta);
        if let Some(ref old_meta) = old {
            self.total_size = self.total_size.saturating_sub(old_meta.size);
        } else {
            self.object_count = self.object_count.saturating_add(1);
        }
        self.total_size = self.total_size.saturating_add(new_size);
        old
    }

    /// Get object metadata by key.
    pub fn get(&self, key: &str) -> Option<&ObjectMeta> {
        self.objects.get(key)
    }

    /// Delete an object by key. Returns the old metadata if it existed.
    pub fn delete(&mut self, key: &str) -> Option<ObjectMeta> {
        let old = self.objects.remove(key);
        if let Some(ref old_meta) = old {
            self.object_count = self.object_count.saturating_sub(1);
            self.total_size = self.total_size.saturating_sub(old_meta.size);
        }
        old
    }

    /// List objects with S3-compatible prefix/delimiter/pagination semantics.
    pub fn list(
        &self,
        prefix: Option<&str>,
        delimiter: Option<&str>,
        start_after: Option<&str>,
        continuation_token: Option<&str>,
        max_keys: u32,
    ) -> ListResult {
        let prefix = prefix.unwrap_or("");
        let max_keys = if max_keys == 0 { 1000 } else { max_keys };

        // Determine the starting point for iteration
        let start_key = continuation_token.or(start_after).unwrap_or("").to_string();

        let mut contents = Vec::new();
        let mut common_prefixes = Vec::new();
        let mut seen_prefixes = std::collections::BTreeSet::new();
        let mut count = 0u32;
        let mut is_truncated = false;
        let mut last_key = String::new();

        // BTreeMap range scan from start_key
        let range = if start_key.is_empty() {
            self.objects.range::<String, _>(..)
        } else {
            // Use exclusive start: skip the start_key itself
            self.objects.range::<String, _>((
                std::ops::Bound::Excluded(start_key),
                std::ops::Bound::Unbounded,
            ))
        };

        for (key, meta) in range {
            // Only include keys that match the prefix
            if !key.starts_with(prefix) {
                // If we've passed the prefix range entirely, stop
                if key.as_str() > prefix && !prefix.is_empty() {
                    // Check if we could still find matches (prefix is a prefix of key range)
                    let prefix_end = increment_last_byte(prefix);
                    if let Some(ref end) = prefix_end {
                        if key.as_str() >= end.as_str() {
                            break;
                        }
                    }
                }
                continue;
            }

            let suffix = &key[prefix.len()..];

            // Handle delimiter grouping
            if let Some(delim) = delimiter {
                if let Some(pos) = suffix.find(delim) {
                    // Group under common prefix
                    let common_prefix = format!("{}{}{}", prefix, &suffix[..pos], delim);
                    if seen_prefixes.insert(common_prefix.clone()) {
                        if count >= max_keys {
                            is_truncated = true;
                            break;
                        }
                        common_prefixes.push(common_prefix);
                        count += 1;
                    }
                    continue;
                }
            }

            if count >= max_keys {
                is_truncated = true;
                break;
            }

            last_key = key.clone();
            contents.push(ObjectEntry {
                key: key.clone(),
                size: meta.size,
                last_modified: meta.last_modified,
                etag: meta.etag.clone(),
            });
            count += 1;
        }

        let next_continuation_token = if is_truncated { Some(last_key) } else { None };

        ListResult {
            contents,
            common_prefixes,
            is_truncated,
            next_continuation_token,
            key_count: count,
        }
    }

    /// Compute a deterministic Merkle root over sorted (key, data_root, size) entries.
    ///
    /// This commitment can be included in checkpoints to prove the metadata state.
    pub fn metadata_merkle_root(&self) -> H256 {
        if self.objects.is_empty() {
            return H256::zero();
        }

        // Build leaf hashes from sorted entries
        let leaves: Vec<H256> = self
            .objects
            .iter()
            .map(|(key, meta)| {
                let mut data = Vec::new();
                data.extend_from_slice(key.as_bytes());
                data.extend_from_slice(meta.data_root.as_bytes());
                data.extend_from_slice(&meta.size.encode());
                blake2_256(&data)
            })
            .collect();

        if leaves.len() == 1 {
            return leaves[0];
        }

        // Pad to power of 2 and build balanced tree
        let padded_len = leaves.len().next_power_of_two();
        let mut current_level = leaves;
        current_level.resize(padded_len, H256::zero());

        while current_level.len() > 1 {
            let mut next_level = Vec::new();
            for pair in current_level.chunks(2) {
                next_level.push(hash_children(pair[0], pair[1]));
            }
            current_level = next_level;
        }

        current_level[0]
    }
}

/// Thread-safe S3 index manager used by ProviderState.
pub struct S3IndexManager {
    indices: DashMap<BucketId, RwLock<BucketIndex>>,
    persistence: Option<IndexPersistence>,
}

impl S3IndexManager {
    /// Create a new manager without persistence.
    pub fn new() -> Self {
        Self {
            indices: DashMap::new(),
            persistence: None,
        }
    }

    /// Create a new manager with disk persistence.
    pub fn with_persistence(data_dir: &Path) -> Result<Self, std::io::Error> {
        let persistence = IndexPersistence::new(data_dir)?;
        let manager = Self {
            indices: DashMap::new(),
            persistence: Some(persistence),
        };
        Ok(manager)
    }

    /// Load all existing indices from disk.
    pub fn load_from_disk(&self) -> Result<usize, std::io::Error> {
        let persistence = match &self.persistence {
            Some(p) => p,
            None => return Ok(0),
        };

        let loaded = persistence.load_all()?;
        let count = loaded.len();
        for (bucket_id, index) in loaded {
            self.indices.insert(bucket_id, RwLock::new(index));
        }
        Ok(count)
    }

    /// Put an object into a bucket's index.
    pub fn put_object(
        &self,
        bucket_id: BucketId,
        key: String,
        meta: ObjectMeta,
    ) -> Option<ObjectMeta> {
        let entry = self
            .indices
            .entry(bucket_id)
            .or_insert_with(|| RwLock::new(BucketIndex::default()));
        let old = entry.write().put(key, meta);
        self.persist(bucket_id);
        old
    }

    /// Get object metadata.
    pub fn get_object(&self, bucket_id: BucketId, key: &str) -> Option<ObjectMeta> {
        self.indices
            .get(&bucket_id)
            .and_then(|entry| entry.read().get(key).cloned())
    }

    /// Delete an object from the index.
    pub fn delete_object(&self, bucket_id: BucketId, key: &str) -> Option<ObjectMeta> {
        let old = self
            .indices
            .get(&bucket_id)
            .and_then(|entry| entry.write().delete(key));
        if old.is_some() {
            self.persist(bucket_id);
        }
        old
    }

    /// List objects in a bucket.
    pub fn list_objects(
        &self,
        bucket_id: BucketId,
        prefix: Option<&str>,
        delimiter: Option<&str>,
        start_after: Option<&str>,
        continuation_token: Option<&str>,
        max_keys: u32,
    ) -> ListResult {
        self.indices
            .get(&bucket_id)
            .map(|entry| {
                entry
                    .read()
                    .list(prefix, delimiter, start_after, continuation_token, max_keys)
            })
            .unwrap_or_else(|| ListResult {
                contents: Vec::new(),
                common_prefixes: Vec::new(),
                is_truncated: false,
                next_continuation_token: None,
                key_count: 0,
            })
    }

    /// Get metadata Merkle root for a bucket.
    pub fn metadata_merkle_root(&self, bucket_id: BucketId) -> H256 {
        self.indices
            .get(&bucket_id)
            .map(|entry| entry.read().metadata_merkle_root())
            .unwrap_or(H256::zero())
    }

    /// Get bucket stats (object_count, total_size).
    pub fn bucket_stats(&self, bucket_id: BucketId) -> (u64, u64) {
        self.indices
            .get(&bucket_id)
            .map(|entry| {
                let idx = entry.read();
                (idx.object_count, idx.total_size)
            })
            .unwrap_or((0, 0))
    }

    /// Persist a bucket's index to disk (best-effort).
    fn persist(&self, bucket_id: BucketId) {
        if let Some(ref persistence) = self.persistence {
            if let Some(entry) = self.indices.get(&bucket_id) {
                let index = entry.read().clone();
                if let Err(e) = persistence.save(bucket_id, &index) {
                    tracing::warn!("Failed to persist S3 index for bucket {}: {}", bucket_id, e);
                }
            }
        }
    }
}

impl Default for S3IndexManager {
    fn default() -> Self {
        Self::new()
    }
}

/// JSON-file-per-bucket persistence.
pub struct IndexPersistence {
    dir: PathBuf,
}

impl IndexPersistence {
    /// Create persistence, ensuring directory exists.
    pub fn new(data_dir: &Path) -> Result<Self, std::io::Error> {
        let dir = data_dir.join("s3_indices");
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    /// Save a bucket index to disk using atomic rename.
    pub fn save(&self, bucket_id: BucketId, index: &BucketIndex) -> Result<(), std::io::Error> {
        let path = self.index_path(bucket_id);
        let tmp_path = path.with_extension("json.tmp");

        let data = serde_json::to_string_pretty(index).map_err(std::io::Error::other)?;

        std::fs::write(&tmp_path, data)?;
        std::fs::rename(&tmp_path, &path)?;
        Ok(())
    }

    /// Load all bucket indices from disk.
    pub fn load_all(&self) -> Result<Vec<(BucketId, BucketIndex)>, std::io::Error> {
        let mut results = Vec::new();

        if !self.dir.exists() {
            return Ok(results);
        }

        for entry in std::fs::read_dir(&self.dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let name_str = name.to_string_lossy();

            // Parse bucket_<id>_index.json
            if let Some(id_str) = name_str
                .strip_prefix("bucket_")
                .and_then(|s| s.strip_suffix("_index.json"))
            {
                if let Ok(bucket_id) = id_str.parse::<BucketId>() {
                    match std::fs::read_to_string(entry.path()) {
                        Ok(data) => match serde_json::from_str::<BucketIndex>(&data) {
                            Ok(index) => {
                                tracing::info!(
                                    "Loaded S3 index for bucket {}: {} objects",
                                    bucket_id,
                                    index.object_count
                                );
                                results.push((bucket_id, index));
                            }
                            Err(e) => {
                                tracing::warn!(
                                    "Failed to parse S3 index for bucket {}: {}",
                                    bucket_id,
                                    e
                                );
                            }
                        },
                        Err(e) => {
                            tracing::warn!(
                                "Failed to read S3 index for bucket {}: {}",
                                bucket_id,
                                e
                            );
                        }
                    }
                }
            }
        }

        Ok(results)
    }

    fn index_path(&self, bucket_id: BucketId) -> PathBuf {
        self.dir.join(format!("bucket_{bucket_id}_index.json"))
    }
}

/// Increment the last byte of a string to create an exclusive upper bound for range scans.
fn increment_last_byte(s: &str) -> Option<String> {
    let mut bytes = s.as_bytes().to_vec();
    if bytes.is_empty() {
        return None;
    }
    if let Some(last) = bytes.last_mut() {
        if *last < 0xFF {
            *last += 1;
            return Some(String::from_utf8_lossy(&bytes).to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_meta(size: u64) -> ObjectMeta {
        ObjectMeta {
            data_root: H256::repeat_byte(1),
            size,
            content_type: "text/plain".to_string(),
            etag: "abc123".to_string(),
            last_modified: 1700000000,
            user_metadata: Vec::new(),
            leaf_index: 0,
        }
    }

    #[test]
    fn test_bucket_index_put_get_delete() {
        let mut idx = BucketIndex::default();

        // Put
        assert!(idx.put("file.txt".to_string(), make_meta(100)).is_none());
        assert_eq!(idx.object_count, 1);
        assert_eq!(idx.total_size, 100);

        // Get
        let meta = idx.get("file.txt").unwrap();
        assert_eq!(meta.size, 100);

        // Overwrite
        let old = idx.put("file.txt".to_string(), make_meta(200));
        assert!(old.is_some());
        assert_eq!(old.unwrap().size, 100);
        assert_eq!(idx.object_count, 1);
        assert_eq!(idx.total_size, 200);

        // Delete
        let removed = idx.delete("file.txt");
        assert!(removed.is_some());
        assert_eq!(idx.object_count, 0);
        assert_eq!(idx.total_size, 0);

        // Delete nonexistent
        assert!(idx.delete("nope").is_none());
    }

    #[test]
    fn test_bucket_index_list_basic() {
        let mut idx = BucketIndex::default();
        idx.put("a.txt".to_string(), make_meta(10));
        idx.put("b.txt".to_string(), make_meta(20));
        idx.put("c.txt".to_string(), make_meta(30));

        let result = idx.list(None, None, None, None, 1000);
        assert_eq!(result.contents.len(), 3);
        assert_eq!(result.key_count, 3);
        assert!(!result.is_truncated);
    }

    #[test]
    fn test_bucket_index_list_prefix() {
        let mut idx = BucketIndex::default();
        idx.put("photos/cat.jpg".to_string(), make_meta(10));
        idx.put("photos/dog.jpg".to_string(), make_meta(20));
        idx.put("docs/readme.txt".to_string(), make_meta(30));

        let result = idx.list(Some("photos/"), None, None, None, 1000);
        assert_eq!(result.contents.len(), 2);
    }

    #[test]
    fn test_bucket_index_list_delimiter() {
        let mut idx = BucketIndex::default();
        idx.put("photos/2024/cat.jpg".to_string(), make_meta(10));
        idx.put("photos/2024/dog.jpg".to_string(), make_meta(20));
        idx.put("photos/2023/old.jpg".to_string(), make_meta(15));
        idx.put("docs/readme.txt".to_string(), make_meta(30));

        // List top-level "folders"
        let result = idx.list(None, Some("/"), None, None, 1000);
        assert_eq!(result.common_prefixes.len(), 2); // "docs/", "photos/"
        assert!(result.contents.is_empty());

        // List within photos/
        let result = idx.list(Some("photos/"), Some("/"), None, None, 1000);
        assert_eq!(result.common_prefixes.len(), 2); // "photos/2023/", "photos/2024/"
        assert!(result.contents.is_empty());
    }

    #[test]
    fn test_bucket_index_list_pagination() {
        let mut idx = BucketIndex::default();
        for i in 0..5 {
            idx.put(format!("file_{i:03}.txt"), make_meta(10));
        }

        // Page 1
        let result = idx.list(None, None, None, None, 2);
        assert_eq!(result.contents.len(), 2);
        assert!(result.is_truncated);
        assert!(result.next_continuation_token.is_some());

        // Page 2
        let token = result.next_continuation_token.unwrap();
        let result = idx.list(None, None, None, Some(&token), 2);
        assert_eq!(result.contents.len(), 2);
        assert!(result.is_truncated);

        // Page 3
        let token = result.next_continuation_token.unwrap();
        let result = idx.list(None, None, None, Some(&token), 2);
        assert_eq!(result.contents.len(), 1);
        assert!(!result.is_truncated);
    }

    #[test]
    fn test_metadata_merkle_root_deterministic() {
        let mut idx = BucketIndex::default();
        idx.put("a.txt".to_string(), make_meta(10));
        idx.put("b.txt".to_string(), make_meta(20));

        let root1 = idx.metadata_merkle_root();
        let root2 = idx.metadata_merkle_root();
        assert_eq!(root1, root2);
        assert_ne!(root1, H256::zero());
    }

    #[test]
    fn test_metadata_merkle_root_empty() {
        let idx = BucketIndex::default();
        assert_eq!(idx.metadata_merkle_root(), H256::zero());
    }

    #[test]
    fn test_persistence_round_trip() {
        let dir = std::env::temp_dir().join(format!("s3_index_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let persistence = IndexPersistence::new(&dir).unwrap();

        let mut index = BucketIndex::default();
        index.put("test.txt".to_string(), make_meta(42));

        persistence.save(1, &index).unwrap();

        let loaded = persistence.load_all().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].0, 1);
        assert_eq!(loaded[0].1.object_count, 1);
        assert_eq!(loaded[0].1.get("test.txt").unwrap().size, 42);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_increment_last_byte_normal() {
        assert_eq!(increment_last_byte("abc"), Some("abd".to_string()));
        assert_eq!(increment_last_byte("z"), Some("{".to_string()));
    }

    #[test]
    fn test_increment_last_byte_empty() {
        assert_eq!(increment_last_byte(""), None);
    }

    #[test]
    fn test_metadata_merkle_root_single_entry() {
        let mut idx = BucketIndex::default();
        idx.put("only.txt".to_string(), make_meta(42));

        let root = idx.metadata_merkle_root();
        assert_ne!(root, H256::zero());
    }

    #[test]
    fn test_s3_index_manager() {
        let manager = S3IndexManager::new();

        // Put
        assert!(manager
            .put_object(1, "hello.txt".to_string(), make_meta(100))
            .is_none());

        // Get
        let meta = manager.get_object(1, "hello.txt").unwrap();
        assert_eq!(meta.size, 100);

        // Stats
        let (count, size) = manager.bucket_stats(1);
        assert_eq!(count, 1);
        assert_eq!(size, 100);

        // List
        let result = manager.list_objects(1, None, None, None, None, 1000);
        assert_eq!(result.contents.len(), 1);

        // Delete
        assert!(manager.delete_object(1, "hello.txt").is_some());
        assert!(manager.get_object(1, "hello.txt").is_none());
    }
}
