// SPDX-License-Identifier: GPL-3.0-only

//! File system drive metadata index.
//!
//! Provides a per-drive sorted path→metadata map for the file system interface.
//! The index is stored in-memory (BTreeMap) with optional JSON persistence to disk.
//! Mirrors the `s3_index.rs` pattern for S3-compatible object storage.

use codec::Encode;
use dashmap::DashMap;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use sp_core::H256;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use storage_primitives::{blake2_256, hash_children, BucketId};

/// Per-entry metadata stored in the FS index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsEntryMeta {
    /// Entry type: "file" or "directory"
    pub entry_type: String,
    /// Merkle tree root of file chunks (files only, H256::zero for directories)
    pub data_root: H256,
    /// Original data size in bytes (0 for directories)
    pub size: u64,
    /// MIME content type (empty for directories)
    pub content_type: String,
    /// Last modified timestamp (Unix epoch seconds)
    pub mtime: u64,
    /// MMR leaf index from commit (files only)
    pub leaf_index: u64,
}

/// An entry returned in directory list responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsListEntry {
    pub name: String,
    pub path: String,
    pub entry_type: String,
    pub size: u64,
    pub mtime: u64,
}

/// Per-drive sorted path→metadata index.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DriveIndex {
    entries: BTreeMap<String, FsEntryMeta>,
    pub file_count: u64,
    pub dir_count: u64,
    pub total_size: u64,
}

impl DriveIndex {
    /// Insert or update a file entry.
    pub fn put_file(&mut self, path: String, meta: FsEntryMeta) -> Option<FsEntryMeta> {
        let new_size = meta.size;
        let old = self.entries.insert(path, meta);
        if let Some(ref old_meta) = old {
            self.total_size = self.total_size.saturating_sub(old_meta.size);
            if old_meta.entry_type == "directory" {
                self.dir_count = self.dir_count.saturating_sub(1);
                self.file_count = self.file_count.saturating_add(1);
            }
        } else {
            self.file_count = self.file_count.saturating_add(1);
        }
        self.total_size = self.total_size.saturating_add(new_size);
        old
    }

    /// Create a directory entry.
    pub fn mkdir(&mut self, path: String) -> Option<FsEntryMeta> {
        let meta = FsEntryMeta {
            entry_type: "directory".to_string(),
            data_root: H256::zero(),
            size: 0,
            content_type: String::new(),
            mtime: current_timestamp(),
            leaf_index: 0,
        };
        let old = self.entries.insert(path, meta);
        if let Some(ref old_meta) = old {
            if old_meta.entry_type == "file" {
                self.file_count = self.file_count.saturating_sub(1);
                self.total_size = self.total_size.saturating_sub(old_meta.size);
                self.dir_count = self.dir_count.saturating_add(1);
            }
        } else {
            self.dir_count = self.dir_count.saturating_add(1);
        }
        old
    }

    /// Get entry metadata by path.
    pub fn get(&self, path: &str) -> Option<&FsEntryMeta> {
        self.entries.get(path)
    }

    /// Delete an entry by path.
    pub fn delete(&mut self, path: &str) -> Option<FsEntryMeta> {
        let old = self.entries.remove(path);
        if let Some(ref old_meta) = old {
            if old_meta.entry_type == "directory" {
                self.dir_count = self.dir_count.saturating_sub(1);
            } else {
                self.file_count = self.file_count.saturating_sub(1);
            }
            self.total_size = self.total_size.saturating_sub(old_meta.size);
        }
        old
    }

    /// List entries under a directory path.
    ///
    /// If `recursive` is false, only returns direct children.
    /// If `recursive` is true, returns all descendants.
    pub fn list(&self, dir_path: &str, recursive: bool) -> Vec<FsListEntry> {
        let prefix = if dir_path == "/" || dir_path.is_empty() {
            "/".to_string()
        } else {
            let normalized = dir_path.trim_end_matches('/');
            format!("{normalized}/")
        };

        let mut results = Vec::new();

        for (path, meta) in self.entries.range::<String, _>(prefix.clone()..) {
            if !path.starts_with(&prefix) {
                break;
            }

            let suffix = &path[prefix.len()..];

            if !recursive {
                // Only direct children: no '/' in the remaining suffix
                if suffix.contains('/') {
                    continue;
                }
            }

            // Extract the entry name (last path component)
            let name = path.rsplit('/').next().unwrap_or(path).to_string();

            results.push(FsListEntry {
                name,
                path: path.clone(),
                entry_type: meta.entry_type.clone(),
                size: meta.size,
                mtime: meta.mtime,
            });
        }

        results
    }

    /// Compute a deterministic Merkle root over sorted (path, data_root, size) entries.
    pub fn metadata_merkle_root(&self) -> H256 {
        if self.entries.is_empty() {
            return H256::zero();
        }

        let leaves: Vec<H256> = self
            .entries
            .iter()
            .map(|(path, meta)| {
                let mut data = Vec::new();
                data.extend_from_slice(path.as_bytes());
                data.extend_from_slice(meta.data_root.as_bytes());
                data.extend_from_slice(&meta.size.encode());
                blake2_256(&data)
            })
            .collect();

        if leaves.len() == 1 {
            return leaves[0];
        }

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

/// Thread-safe, cloneable handle to the per-drive file-system indexes,
/// intended to be shared across request handlers.
pub struct FsIndexManager {
    indices: DashMap<BucketId, RwLock<DriveIndex>>,
    persistence: Option<FsIndexPersistence>,
}

impl FsIndexManager {
    /// Create a new manager without persistence.
    pub fn new() -> Self {
        Self {
            indices: DashMap::new(),
            persistence: None,
        }
    }

    /// Create a new manager with disk persistence.
    pub fn with_persistence(data_dir: &Path) -> Result<Self, std::io::Error> {
        let persistence = FsIndexPersistence::new(data_dir)?;
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

    /// Put a file into a drive's index.
    pub fn put_file(
        &self,
        bucket_id: BucketId,
        path: String,
        meta: FsEntryMeta,
    ) -> Option<FsEntryMeta> {
        let entry = self
            .indices
            .entry(bucket_id)
            .or_insert_with(|| RwLock::new(DriveIndex::default()));
        let old = entry.write().put_file(path, meta);
        self.persist(bucket_id);
        old
    }

    /// Create a directory in a drive's index.
    pub fn mkdir(&self, bucket_id: BucketId, path: String) -> Option<FsEntryMeta> {
        let entry = self
            .indices
            .entry(bucket_id)
            .or_insert_with(|| RwLock::new(DriveIndex::default()));
        let old = entry.write().mkdir(path);
        self.persist(bucket_id);
        old
    }

    /// Get entry metadata.
    pub fn get_entry(&self, bucket_id: BucketId, path: &str) -> Option<FsEntryMeta> {
        self.indices
            .get(&bucket_id)
            .and_then(|entry| entry.read().get(path).cloned())
    }

    /// Delete an entry from the index.
    pub fn delete_entry(&self, bucket_id: BucketId, path: &str) -> Option<FsEntryMeta> {
        let old = self
            .indices
            .get(&bucket_id)
            .and_then(|entry| entry.write().delete(path));
        if old.is_some() {
            self.persist(bucket_id);
        }
        old
    }

    /// List entries under a directory path.
    pub fn list_dir(&self, bucket_id: BucketId, path: &str, recursive: bool) -> Vec<FsListEntry> {
        self.indices
            .get(&bucket_id)
            .map(|entry| entry.read().list(path, recursive))
            .unwrap_or_default()
    }

    /// Get metadata Merkle root for a drive.
    pub fn metadata_merkle_root(&self, bucket_id: BucketId) -> H256 {
        self.indices
            .get(&bucket_id)
            .map(|entry| entry.read().metadata_merkle_root())
            .unwrap_or(H256::zero())
    }

    /// Get drive stats (file_count, dir_count, total_size).
    pub fn drive_stats(&self, bucket_id: BucketId) -> (u64, u64, u64) {
        self.indices
            .get(&bucket_id)
            .map(|entry| {
                let idx = entry.read();
                (idx.file_count, idx.dir_count, idx.total_size)
            })
            .unwrap_or((0, 0, 0))
    }

    /// Persist a drive's index to disk (best-effort).
    fn persist(&self, bucket_id: BucketId) {
        if let Some(ref persistence) = self.persistence {
            if let Some(entry) = self.indices.get(&bucket_id) {
                let index = entry.read().clone();
                if let Err(e) = persistence.save(bucket_id, &index) {
                    tracing::warn!("Failed to persist FS index for bucket {}: {}", bucket_id, e);
                }
            }
        }
    }
}

impl Default for FsIndexManager {
    fn default() -> Self {
        Self::new()
    }
}

/// JSON-file-per-drive persistence.
pub struct FsIndexPersistence {
    dir: PathBuf,
}

impl FsIndexPersistence {
    /// Create persistence, ensuring directory exists.
    pub fn new(data_dir: &Path) -> Result<Self, std::io::Error> {
        let dir = data_dir.join("fs_indices");
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    /// Save a drive index to disk using atomic rename.
    pub fn save(&self, bucket_id: BucketId, index: &DriveIndex) -> Result<(), std::io::Error> {
        let path = self.index_path(bucket_id);
        let tmp_path = path.with_extension("json.tmp");

        let data = serde_json::to_string_pretty(index).map_err(std::io::Error::other)?;

        std::fs::write(&tmp_path, data)?;
        std::fs::rename(&tmp_path, &path)?;
        Ok(())
    }

    /// Load all drive indices from disk.
    pub fn load_all(&self) -> Result<Vec<(BucketId, DriveIndex)>, std::io::Error> {
        let mut results = Vec::new();

        if !self.dir.exists() {
            return Ok(results);
        }

        for entry in std::fs::read_dir(&self.dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let name_str = name.to_string_lossy();

            // Parse drive_<id>_index.json
            if let Some(id_str) = name_str
                .strip_prefix("drive_")
                .and_then(|s| s.strip_suffix("_index.json"))
            {
                if let Ok(bucket_id) = id_str.parse::<BucketId>() {
                    match std::fs::read_to_string(entry.path()) {
                        Ok(data) => match serde_json::from_str::<DriveIndex>(&data) {
                            Ok(index) => {
                                tracing::info!(
                                    "Loaded FS index for drive (bucket {}): {} files, {} dirs",
                                    bucket_id,
                                    index.file_count,
                                    index.dir_count,
                                );
                                results.push((bucket_id, index));
                            }
                            Err(e) => {
                                tracing::warn!(
                                    "Failed to parse FS index for bucket {}: {}",
                                    bucket_id,
                                    e
                                );
                            }
                        },
                        Err(e) => {
                            tracing::warn!(
                                "Failed to read FS index for bucket {}: {}",
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
        self.dir.join(format!("drive_{bucket_id}_index.json"))
    }
}

fn current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_file_meta(size: u64) -> FsEntryMeta {
        FsEntryMeta {
            entry_type: "file".to_string(),
            data_root: H256::repeat_byte(1),
            size,
            content_type: "text/plain".to_string(),
            mtime: 1700000000,
            leaf_index: 0,
        }
    }

    #[test]
    fn test_drive_index_put_get_delete() {
        let mut idx = DriveIndex::default();

        // Put file
        assert!(idx
            .put_file("/hello.txt".to_string(), make_file_meta(100))
            .is_none());
        assert_eq!(idx.file_count, 1);
        assert_eq!(idx.total_size, 100);

        // Get
        let meta = idx.get("/hello.txt").unwrap();
        assert_eq!(meta.size, 100);

        // Overwrite
        let old = idx.put_file("/hello.txt".to_string(), make_file_meta(200));
        assert!(old.is_some());
        assert_eq!(old.unwrap().size, 100);
        assert_eq!(idx.file_count, 1);
        assert_eq!(idx.total_size, 200);

        // Delete
        let removed = idx.delete("/hello.txt");
        assert!(removed.is_some());
        assert_eq!(idx.file_count, 0);
        assert_eq!(idx.total_size, 0);

        // Delete nonexistent
        assert!(idx.delete("/nope").is_none());
    }

    #[test]
    fn test_drive_index_mkdir() {
        let mut idx = DriveIndex::default();

        assert!(idx.mkdir("/docs".to_string()).is_none());
        assert_eq!(idx.dir_count, 1);
        assert_eq!(idx.file_count, 0);

        let meta = idx.get("/docs").unwrap();
        assert_eq!(meta.entry_type, "directory");
    }

    #[test]
    fn test_drive_index_list() {
        let mut idx = DriveIndex::default();
        idx.mkdir("/docs".to_string());
        idx.put_file("/docs/a.txt".to_string(), make_file_meta(10));
        idx.put_file("/docs/b.txt".to_string(), make_file_meta(20));
        idx.mkdir("/docs/sub".to_string());
        idx.put_file("/docs/sub/c.txt".to_string(), make_file_meta(30));
        idx.put_file("/other.txt".to_string(), make_file_meta(5));

        // List direct children of /docs
        let entries = idx.list("/docs", false);
        assert_eq!(entries.len(), 3); // a.txt, b.txt, sub

        // List recursive
        let entries = idx.list("/docs", true);
        assert_eq!(entries.len(), 4); // a.txt, b.txt, sub, sub/c.txt

        // List root
        let entries = idx.list("/", false);
        assert_eq!(entries.len(), 2); // docs, other.txt
    }

    #[test]
    fn test_metadata_merkle_root_deterministic() {
        let mut idx = DriveIndex::default();
        idx.put_file("/a.txt".to_string(), make_file_meta(10));
        idx.put_file("/b.txt".to_string(), make_file_meta(20));

        let root1 = idx.metadata_merkle_root();
        let root2 = idx.metadata_merkle_root();
        assert_eq!(root1, root2);
        assert_ne!(root1, H256::zero());
    }

    #[test]
    fn test_metadata_merkle_root_empty() {
        let idx = DriveIndex::default();
        assert_eq!(idx.metadata_merkle_root(), H256::zero());
    }

    #[test]
    fn test_persistence_round_trip() {
        let dir = std::env::temp_dir().join(format!("fs_index_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let persistence = FsIndexPersistence::new(&dir).unwrap();

        let mut index = DriveIndex::default();
        index.put_file("/test.txt".to_string(), make_file_meta(42));
        index.mkdir("/docs".to_string());

        persistence.save(1, &index).unwrap();

        let loaded = persistence.load_all().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].0, 1);
        assert_eq!(loaded[0].1.file_count, 1);
        assert_eq!(loaded[0].1.dir_count, 1);
        assert_eq!(loaded[0].1.get("/test.txt").unwrap().size, 42);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_file_to_dir_transition() {
        let mut idx = DriveIndex::default();
        idx.put_file("/path".to_string(), make_file_meta(100));
        assert_eq!(idx.file_count, 1);
        assert_eq!(idx.dir_count, 0);
        assert_eq!(idx.total_size, 100);

        // Replace file with directory
        idx.mkdir("/path".to_string());
        assert_eq!(idx.file_count, 0);
        assert_eq!(idx.dir_count, 1);
        assert_eq!(idx.total_size, 0);
    }

    #[test]
    fn test_dir_to_file_transition() {
        let mut idx = DriveIndex::default();
        idx.mkdir("/path".to_string());
        assert_eq!(idx.dir_count, 1);
        assert_eq!(idx.file_count, 0);

        // Replace directory with file
        idx.put_file("/path".to_string(), make_file_meta(200));
        assert_eq!(idx.dir_count, 0);
        assert_eq!(idx.file_count, 1);
        assert_eq!(idx.total_size, 200);
    }

    #[test]
    fn test_metadata_merkle_root_single_entry() {
        let mut idx = DriveIndex::default();
        idx.put_file("/only.txt".to_string(), make_file_meta(42));

        let root = idx.metadata_merkle_root();
        assert_ne!(root, H256::zero());
    }

    #[test]
    fn test_fs_index_manager() {
        let manager = FsIndexManager::new();

        // Put file
        assert!(manager
            .put_file(1, "/hello.txt".to_string(), make_file_meta(100))
            .is_none());

        // Get
        let meta = manager.get_entry(1, "/hello.txt").unwrap();
        assert_eq!(meta.size, 100);

        // Mkdir
        manager.mkdir(1, "/docs".to_string());

        // Stats
        let (files, dirs, size) = manager.drive_stats(1);
        assert_eq!(files, 1);
        assert_eq!(dirs, 1);
        assert_eq!(size, 100);

        // List
        let entries = manager.list_dir(1, "/", false);
        assert_eq!(entries.len(), 2); // hello.txt, docs

        // Delete
        assert!(manager.delete_entry(1, "/hello.txt").is_some());
        assert!(manager.get_entry(1, "/hello.txt").is_none());
    }
}
