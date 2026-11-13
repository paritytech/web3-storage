// SPDX-License-Identifier: Apache-2.0

//! Blob format of the file system.
//!
//! The file system and S3 clients store every directory and file manifest as
//! a blob in the drive's bucket: a header ([`FORMAT_MAGIC`] and
//! [`FORMAT_VERSION`]) and the SCALE encoding.
//!
//! - **DirectoryNode**: a directory and the references to its children
//! - **FileManifest**: a file's size, content type and chunks
//! - **CID**: Content Identifier of a blob: its `data_root`
//!   (see [`compute_cid`])
//!
//! The root directory is the data root of the bucket's last MMR leaf.

use bounded_collections::{BoundedVec, Get};
use codec::{Decode, DecodeAll, Encode, MaxEncodedLen};
use primitive_types::H256;
use storage_primitives::BucketId;

// ============================================================================
// SCALE-encoded types
// ============================================================================

/// Content Identifier: the `data_root` of a blob (see [`compute_cid`]).
pub type Cid = H256;

/// Content type stored when the uploader gives none.
pub const DEFAULT_MIME_TYPE: &str = "application/octet-stream";

/// Chunk size of the [`compute_cid`] rule, in bytes (256 KiB).
pub const CHUNK_SIZE: usize = storage_primitives::DEFAULT_CHUNK_SIZE as usize;

/// Entry type enumeration
#[derive(Clone, Copy, Encode, Decode, Default, Eq, PartialEq, Debug, MaxEncodedLen)]
pub enum EntryType {
    /// A file entry
    #[codec(index = 0)]
    #[default]
    File,
    /// A directory entry
    #[codec(index = 1)]
    Directory,
}

/// Maximum length for entry names (256 bytes)
pub struct MaxEntryNameLength;
impl Get<u32> for MaxEntryNameLength {
    fn get() -> u32 {
        256
    }
}

/// Maximum number of children in a directory (1024)
pub struct MaxDirectoryChildren;
impl Get<u32> for MaxDirectoryChildren {
    fn get() -> u32 {
        1024
    }
}

/// Maximum number of metadata entries (64)
pub struct MaxMetadataEntries;
impl Get<u32> for MaxMetadataEntries {
    fn get() -> u32 {
        64
    }
}

/// Maximum length for metadata keys (64 bytes)
pub struct MaxMetadataKeyLength;
impl Get<u32> for MaxMetadataKeyLength {
    fn get() -> u32 {
        64
    }
}

/// Maximum length for metadata values (256 bytes)
pub struct MaxMetadataValueLength;
impl Get<u32> for MaxMetadataValueLength {
    fn get() -> u32 {
        256
    }
}

/// Maximum number of chunks in a file (65536)
pub struct MaxFileChunks;
impl Get<u32> for MaxFileChunks {
    fn get() -> u32 {
        65536
    }
}

/// Maximum length for MIME type strings (128 bytes)
pub struct MaxMimeTypeLength;
impl Get<u32> for MaxMimeTypeLength {
    fn get() -> u32 {
        128
    }
}

/// Maximum length for encryption params (512 bytes)
pub struct MaxEncryptionParamsLength;
impl Get<u32> for MaxEncryptionParamsLength {
    fn get() -> u32 {
        512
    }
}

/// A single entry in a directory.
///
/// `name` is 1..=256 bytes of UTF-8, contains no `/`, and is not `.` or `..`
/// (see [`validate_entry_name`]). For a file, `cid` is the CID of its
/// [`FileManifest`] and `size` the content size; for a directory, `cid` is
/// the CID of its [`DirectoryNode`] and `size` is 0.
#[derive(Clone, Encode, Decode, Eq, PartialEq, Debug, MaxEncodedLen)]
pub struct DirectoryEntry {
    /// Human-readable name
    pub name: BoundedVec<u8, MaxEntryNameLength>,
    /// File or Directory
    pub entry_type: EntryType,
    /// CID of the file's manifest or the directory's node
    pub cid: Cid,
    /// Content size in bytes for a file, 0 for a directory
    pub size: u64,
    /// Modification timestamp (Unix timestamp)
    pub mtime: u64,
}

impl DirectoryEntry {
    /// Create a directory entry. Fails with
    /// [`FileSystemError::InvalidEntryName`] if `name` breaks the rules of
    /// [`validate_entry_name`].
    pub fn try_new(
        name: &str,
        entry_type: EntryType,
        cid: Cid,
        size: u64,
        mtime: u64,
    ) -> Result<Self, FileSystemError> {
        validate_entry_name(name.as_bytes())?;
        Ok(Self {
            name: BoundedVec::try_from(name.as_bytes().to_vec())
                .map_err(|_| FileSystemError::InvalidEntryName)?,
            entry_type,
            cid,
            size,
            mtime,
        })
    }

    /// Get the name as a string (lossy conversion)
    pub fn name_str(&self) -> String {
        String::from_utf8_lossy(&self.name).into_owned()
    }

    /// Check if this entry is a directory
    pub fn is_directory(&self) -> bool {
        self.entry_type == EntryType::Directory
    }

    /// Check if this entry is a file
    pub fn is_file(&self) -> bool {
        self.entry_type == EntryType::File
    }
}

/// Metadata key-value pair: key up to 64 bytes, value up to 256 bytes.
#[derive(Clone, Encode, Decode, Eq, PartialEq, Debug, MaxEncodedLen)]
pub struct MetadataEntry {
    /// Metadata key
    pub key: BoundedVec<u8, MaxMetadataKeyLength>,
    /// Metadata value
    pub value: BoundedVec<u8, MaxMetadataValueLength>,
}

impl MetadataEntry {
    /// Create an entry. Fails with [`FileSystemError::MetadataTooLarge`] if the
    /// key is over 64 bytes or the value over 256 bytes.
    pub fn try_new(key: &[u8], value: &[u8]) -> Result<Self, FileSystemError> {
        Ok(Self {
            key: BoundedVec::try_from(key.to_vec())
                .map_err(|_| FileSystemError::MetadataTooLarge)?,
            value: BoundedVec::try_from(value.to_vec())
                .map_err(|_| FileSystemError::MetadataTooLarge)?,
        })
    }
}

/// First bytes of every [`DirectoryNode`] and [`FileManifest`] blob.
pub const FORMAT_MAGIC: [u8; 4] = *b"W3FS";

/// Version of the blob format. A reader rejects blobs of other versions
/// with [`FileSystemError::UnsupportedVersion`].
pub const FORMAT_VERSION: u8 = 1;

/// Length of the blob header: [`FORMAT_MAGIC`] and [`FORMAT_VERSION`].
pub const HEADER_LEN: usize = 5;

/// The blob of `value`: the header, then the SCALE encoding.
fn to_blob(value: &impl Encode) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(HEADER_LEN + value.size_hint());
    bytes.extend(FORMAT_MAGIC);
    bytes.push(FORMAT_VERSION);
    value.encode_to(&mut bytes);
    bytes
}

/// Decode a blob written by [`to_blob`]. Fails with
/// [`FileSystemError::UnsupportedVersion`] for another format version and
/// with [`FileSystemError::NotAFormatBlob`] for anything else that is not
/// exactly such a blob, trailing bytes included.
fn from_blob<T: Decode>(bytes: &[u8]) -> Result<T, FileSystemError> {
    let (header, mut body) = bytes
        .split_at_checked(HEADER_LEN)
        .ok_or(FileSystemError::NotAFormatBlob)?;
    if header[..4] != FORMAT_MAGIC {
        return Err(FileSystemError::NotAFormatBlob);
    }
    if header[4] != FORMAT_VERSION {
        return Err(FileSystemError::UnsupportedVersion(header[4]));
    }
    T::decode_all(&mut body).map_err(|_| FileSystemError::NotAFormatBlob)
}

/// Directory node containing child references.
///
/// `children` are sorted by name bytes, ascending, and names are unique.
/// `metadata` is reserved and empty. [`DirectoryNode::validate`] checks both.
#[derive(Clone, Encode, Decode, Eq, PartialEq, Debug, MaxEncodedLen)]
pub struct DirectoryNode {
    /// Bucket id
    pub bucket_id: BucketId,
    /// Child entries, sorted by name bytes
    pub children: BoundedVec<DirectoryEntry, MaxDirectoryChildren>,
    /// Reserved, empty
    pub metadata: BoundedVec<MetadataEntry, MaxMetadataEntries>,
}

impl DirectoryNode {
    /// Create a new empty directory
    pub fn new_empty(bucket_id: BucketId) -> Self {
        Self {
            bucket_id,
            children: BoundedVec::default(),
            metadata: BoundedVec::default(),
        }
    }

    /// Insert `entry`, or replace the child with the same name. Keeps the
    /// children sorted. Fails with [`FileSystemError::DirectoryFull`] if the
    /// directory already has 1024 children.
    pub fn upsert_child(&mut self, entry: DirectoryEntry) -> Result<(), FileSystemError> {
        match self.position(&entry.name) {
            Ok(pos) => {
                self.children[pos] = entry;
                Ok(())
            }
            Err(pos) => self
                .children
                .try_insert(pos, entry)
                .map_err(|_| FileSystemError::DirectoryFull),
        }
    }

    /// Find a child by name
    pub fn find_child(&self, name: &str) -> Option<&DirectoryEntry> {
        self.position(name.as_bytes())
            .ok()
            .map(|pos| &self.children[pos])
    }

    /// Remove a child by name
    pub fn remove_child(&mut self, name: &str) -> Option<DirectoryEntry> {
        self.position(name.as_bytes())
            .ok()
            .map(|pos| self.children.remove(pos))
    }

    /// Binary search for `name` in the sorted children.
    fn position(&self, name: &[u8]) -> Result<usize, usize> {
        self.children
            .binary_search_by(|e| e.name.as_slice().cmp(name))
    }

    /// Check the rules SCALE decoding does not: valid, sorted, unique child
    /// names, size 0 on directory entries, and empty `metadata`.
    pub fn validate(&self) -> Result<(), FileSystemError> {
        if !self.metadata.is_empty() {
            return Err(FileSystemError::InvalidDirectory);
        }
        for entry in self.children.iter() {
            validate_entry_name(&entry.name)?;
            if entry.is_directory() && entry.size != 0 {
                return Err(FileSystemError::InvalidDirectory);
            }
        }
        if self
            .children
            .windows(2)
            .any(|pair| pair[0].name.as_slice() >= pair[1].name.as_slice())
        {
            return Err(FileSystemError::InvalidDirectory);
        }
        Ok(())
    }

    /// The blob of this node: the header, then the SCALE encoding.
    pub fn to_blob(&self) -> Vec<u8> {
        to_blob(self)
    }

    /// Decode a blob written by [`DirectoryNode::to_blob`] (see
    /// [`FileSystemError::NotAFormatBlob`]).
    pub fn from_blob(bytes: &[u8]) -> Result<Self, FileSystemError> {
        from_blob(bytes)
    }

    /// Largest blob of a directory node: the header and the largest SCALE
    /// encoding.
    pub fn max_blob_len() -> usize {
        HEADER_LEN + Self::max_encoded_len()
    }

    /// CID of the blob (see [`compute_cid`])
    pub fn compute_cid(&self) -> Cid {
        compute_cid(&self.to_blob())
    }
}

/// A single chunk reference in a file
#[derive(Clone, Encode, Decode, Eq, PartialEq, Debug, MaxEncodedLen)]
pub struct FileChunk {
    /// Merkle leaf hash of the chunk
    pub cid: Cid,
    /// Position in the file (0-indexed)
    pub sequence: u32,
}

/// File manifest tracking how to reassemble a file from chunks.
///
/// `chunks` lists the hashes of the content's 256 KiB chunks in order, with
/// `sequence` equal to the index. Empty content has no chunks and no stored
/// blob, because the provider stores no chunk tree for empty data. The content's data
/// root is [`FileManifest::content_root`].
#[derive(Clone, Encode, Decode, Eq, PartialEq, Debug, MaxEncodedLen)]
pub struct FileManifest {
    /// Bucket id
    pub bucket_id: BucketId,
    /// MIME type (e.g., "image/png"); [`DEFAULT_MIME_TYPE`] if none was given
    pub mime_type: BoundedVec<u8, MaxMimeTypeLength>,
    /// Total file size in bytes
    pub total_size: u64,
    /// Ordered list of chunks
    pub chunks: BoundedVec<FileChunk, MaxFileChunks>,
    /// Encryption parameters (optional, for W3ACL); empty
    pub encryption_params: BoundedVec<u8, MaxEncryptionParamsLength>,
    /// User metadata (for example S3 `x-amz-meta-*` values)
    pub user_metadata: BoundedVec<MetadataEntry, MaxMetadataEntries>,
}

impl FileManifest {
    /// Build the manifest of `content`: its chunk hashes, size, `mime_type`
    /// ([`DEFAULT_MIME_TYPE`] if empty) and `user_metadata`, sorted by key
    /// bytes. Fails with [`FileSystemError::ContentTooLarge`] if the content
    /// has more than 65536 chunks, with [`FileSystemError::MetadataTooLarge`]
    /// if the MIME type is over 128 bytes or there are more than 64 metadata
    /// entries, and with [`FileSystemError::DuplicateMetadataKey`] if two
    /// entries have the same key.
    pub fn for_content(
        bucket_id: BucketId,
        content: &[u8],
        mime_type: &str,
        mut user_metadata: Vec<MetadataEntry>,
    ) -> Result<Self, FileSystemError> {
        user_metadata.sort_by(|a, b| a.key.cmp(&b.key));
        if user_metadata
            .windows(2)
            .any(|pair| pair[0].key == pair[1].key)
        {
            return Err(FileSystemError::DuplicateMetadataKey);
        }
        let chunks = chunk_hashes(content)
            .into_iter()
            .enumerate()
            .map(|(i, cid)| FileChunk {
                cid,
                sequence: i as u32,
            })
            .collect::<Vec<_>>();
        let mime_type = if mime_type.is_empty() {
            DEFAULT_MIME_TYPE
        } else {
            mime_type
        };
        Ok(Self {
            bucket_id,
            mime_type: BoundedVec::try_from(mime_type.as_bytes().to_vec())
                .map_err(|_| FileSystemError::MetadataTooLarge)?,
            total_size: content.len() as u64,
            chunks: BoundedVec::try_from(chunks).map_err(|_| FileSystemError::ContentTooLarge)?,
            encryption_params: BoundedVec::default(),
            user_metadata: BoundedVec::try_from(user_metadata)
                .map_err(|_| FileSystemError::MetadataTooLarge)?,
        })
    }

    /// Data root of the content: the Merkle root over the chunk hashes.
    /// Equals [`compute_cid`] of the content.
    pub fn content_root(&self) -> Cid {
        let leaves: Vec<H256> = self.chunks.iter().map(|c| c.cid).collect();
        storage_primitives::merkle_root(&leaves)
    }

    /// The MIME type as a string; [`DEFAULT_MIME_TYPE`] if the stored one is
    /// empty.
    pub fn mime_type_str(&self) -> String {
        if self.mime_type.is_empty() {
            return DEFAULT_MIME_TYPE.into();
        }
        String::from_utf8_lossy(&self.mime_type).into_owned()
    }

    /// Check the rules SCALE decoding does not: `chunks` has one entry per
    /// [`CHUNK_SIZE`] chunk of `total_size` bytes (none for empty content),
    /// each chunk's `sequence` is its index, and `user_metadata` is sorted by
    /// key bytes with unique keys.
    pub fn validate(&self) -> Result<(), FileSystemError> {
        if self
            .user_metadata
            .windows(2)
            .any(|pair| pair[0].key >= pair[1].key)
        {
            return Err(FileSystemError::InvalidManifest);
        }
        let expected = self.total_size.div_ceil(CHUNK_SIZE as u64);
        if self.chunks.len() as u64 != expected {
            return Err(FileSystemError::InvalidManifest);
        }
        if self
            .chunks
            .iter()
            .enumerate()
            .any(|(i, chunk)| chunk.sequence as usize != i)
        {
            return Err(FileSystemError::InvalidManifest);
        }
        Ok(())
    }

    /// The blob of this manifest: the header, then the SCALE encoding.
    pub fn to_blob(&self) -> Vec<u8> {
        to_blob(self)
    }

    /// Decode a blob written by [`FileManifest::to_blob`] (see
    /// [`FileSystemError::NotAFormatBlob`]).
    pub fn from_blob(bytes: &[u8]) -> Result<Self, FileSystemError> {
        from_blob(bytes)
    }

    /// Largest blob of a file manifest: the header and the largest SCALE
    /// encoding.
    pub fn max_blob_len() -> usize {
        HEADER_LEN + Self::max_encoded_len()
    }

    /// CID of the blob (see [`compute_cid`])
    pub fn compute_cid(&self) -> Cid {
        compute_cid(&self.to_blob())
    }
}

// ============================================================================
// Error types
// ============================================================================

/// Errors of the file-system format: building a node or manifest that breaks
/// a bound, or reading one that breaks the format rules.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FileSystemError {
    /// An entry name is empty, over 256 bytes, not UTF-8, contains `/`, or is
    /// `.` or `..`.
    #[error("Invalid entry name")]
    InvalidEntryName,
    /// A directory node has unsorted or duplicate child names, a directory
    /// entry with a non-zero size, or non-empty metadata.
    #[error("Invalid directory node")]
    InvalidDirectory,
    /// A file manifest's chunk count does not match its `total_size`, a
    /// chunk's `sequence` is not its index, or its user metadata is not
    /// sorted by key with unique keys.
    #[error("Invalid file manifest")]
    InvalidManifest,
    /// The directory already has 1024 children, the most a node holds.
    #[error("Directory is full")]
    DirectoryFull,
    /// The content has more than 65536 chunks (16 GiB), the most a manifest
    /// lists.
    #[error("Content is too large")]
    ContentTooLarge,
    /// The MIME type is over 128 bytes, there are more than 64 metadata
    /// entries, or a metadata key or value is over 64 or 256 bytes.
    #[error("File metadata exceeds a size bound")]
    MetadataTooLarge,
    /// Two user metadata entries have the same key.
    #[error("Duplicate metadata key")]
    DuplicateMetadataKey,
    /// The bytes do not start with the format header, or are not exactly the
    /// encoding of the node.
    #[error("Not a file-system blob")]
    NotAFormatBlob,
    /// The blob has a format version this code does not read.
    #[error("Unsupported format version {0}")]
    UnsupportedVersion(u8),
}

// ============================================================================
// Utility functions
// ============================================================================

/// CID of a blob: its `data_root`.
///
/// Splits `data` into 256 KiB chunks, hashes each chunk as a Merkle leaf, and
/// returns `storage_primitives::merkle_root` over the chunk hashes. For a blob
/// of at most 256 KiB this is the leaf hash of the blob. Empty data has no
/// chunks, so its CID is the zero hash and no blob exists for it.
pub fn compute_cid(data: &[u8]) -> Cid {
    storage_primitives::merkle_root(&chunk_hashes(data))
}

/// Leaf hashes of the 256 KiB chunks of `data`, in order. Empty data has no
/// chunks.
pub fn chunk_hashes(data: &[u8]) -> Vec<Cid> {
    data.chunks(CHUNK_SIZE)
        .map(storage_primitives::hash_leaf)
        .collect()
}

/// Check an entry name: 1..=256 bytes of UTF-8, no `/`, not `.` or `..`.
pub fn validate_entry_name(name: &[u8]) -> Result<(), FileSystemError> {
    let valid = !name.is_empty()
        && name.len() <= MaxEntryNameLength::get() as usize
        && core::str::from_utf8(name).is_ok()
        && !name.contains(&b'/')
        && name != b"."
        && name != b"..";
    if valid {
        Ok(())
    } else {
        Err(FileSystemError::InvalidEntryName)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn h(data: &[u8]) -> H256 {
        storage_primitives::hash_leaf(data)
    }

    fn entry(name: &str, entry_type: EntryType, size: u64) -> DirectoryEntry {
        DirectoryEntry::try_new(name, entry_type, h(name.as_bytes()), size, 1).unwrap()
    }

    /// Readers request directory and manifest blobs up to these sizes
    /// ([`crate::tree::BlobLength::AtMost`]), so a change to them changes
    /// which blobs a reader accepts.
    #[test]
    fn max_blob_lengths_are_pinned() {
        assert_eq!(DirectoryNode::max_blob_len(), 335_121);
        assert_eq!(FileManifest::max_blob_len(), 2_380_703);
    }

    /// Pinned encoding and CID: a change to the format changes them.
    #[test]
    fn directory_node_test_vector() {
        let mut dir = DirectoryNode::new_empty(7);
        dir.upsert_child(
            DirectoryEntry::try_new("docs", EntryType::Directory, h(b"d"), 0, 1_700_000_001)
                .unwrap(),
        )
        .unwrap();
        dir.upsert_child(
            DirectoryEntry::try_new("a.txt", EntryType::File, h(b"a"), 1, 1_700_000_000).unwrap(),
        )
        .unwrap();
        assert_eq!(dir.children[0].name_str(), "a.txt");
        assert_eq!(
            array_bytes::bytes2hex("", dir.to_blob()),
            DIRECTORY_NODE_HEX,
            "encoding"
        );
        assert_eq!(
            format!("{:#x}", dir.compute_cid()),
            DIRECTORY_NODE_CID,
            "cid"
        );
    }

    /// Pinned encoding and CID: a change to the format changes them.
    #[test]
    fn file_manifest_test_vector() {
        let manifest = FileManifest::for_content(
            7,
            b"a",
            "text/plain",
            vec![MetadataEntry::try_new(b"origin", b"test").unwrap()],
        )
        .unwrap();
        assert_eq!(manifest.chunks[0].cid, h(b"a"));
        assert_eq!(manifest.content_root(), h(b"a"));
        assert_eq!(
            array_bytes::bytes2hex("", manifest.to_blob()),
            FILE_MANIFEST_HEX,
            "encoding"
        );
        assert_eq!(
            format!("{:#x}", manifest.compute_cid()),
            FILE_MANIFEST_CID,
            "cid"
        );
    }

    const DIRECTORY_NODE_HEX: &str = "573346530107000000000000000814612e747874007234082e1dd0b5ec0acd71875d61c9f374af30c100bc4de7aa4eb3f15bbed686010000000000000000f153650000000010646f6373019fa96117cda5187dd7699a57a16cbf37bb3353c1385e7854ab6e131df63cc659000000000000000001f153650000000000";
    const DIRECTORY_NODE_CID: &str =
        "0xccfa3e63456971a857964b18a52e7dbbd498368b0caf5b2a0688dfebe2e7674c";
    const FILE_MANIFEST_HEX: &str = "5733465301070000000000000028746578742f706c61696e0100000000000000047234082e1dd0b5ec0acd71875d61c9f374af30c100bc4de7aa4eb3f15bbed686000000000004186f726967696e1074657374";
    const FILE_MANIFEST_CID: &str =
        "0x1899737ddced289884db8a8926f994dc0d7cfd352fb1148cb8cd42d8d526d551";

    #[test]
    fn compute_cid_is_the_data_root() {
        assert_eq!(compute_cid(b""), H256::zero());
        assert_eq!(compute_cid(b"hello"), h(b"hello"));
        let exact = vec![1u8; CHUNK_SIZE];
        assert_eq!(compute_cid(&exact), h(&exact));

        let data: Vec<u8> = (0..CHUNK_SIZE * 2 + 5).map(|i| i as u8).collect();
        let leaves = vec![
            h(&data[..CHUNK_SIZE]),
            h(&data[CHUNK_SIZE..CHUNK_SIZE * 2]),
            h(&data[CHUNK_SIZE * 2..]),
        ];
        assert_eq!(chunk_hashes(&data), leaves);
        let expected = storage_primitives::hash_children(
            storage_primitives::hash_children(leaves[0], leaves[1]),
            leaves[2],
        );
        assert_eq!(compute_cid(&data), expected);
        assert_ne!(compute_cid(&data), h(&data));
    }

    #[test]
    fn manifest_of_empty_content_has_no_chunks() {
        let manifest = FileManifest::for_content(1, b"", DEFAULT_MIME_TYPE, vec![]).unwrap();
        assert_eq!(manifest.total_size, 0);
        assert!(manifest.chunks.is_empty());
        assert_eq!(manifest.content_root(), compute_cid(b""));
    }

    #[test]
    fn manifest_content_root_matches_content_cid() {
        let data: Vec<u8> = (0..CHUNK_SIZE * 3 + 1).map(|i| (i % 7) as u8).collect();
        let manifest = FileManifest::for_content(1, &data, DEFAULT_MIME_TYPE, vec![]).unwrap();
        assert_eq!(manifest.chunks.len(), 4);
        assert_eq!(manifest.chunks[3].sequence, 3);
        assert_eq!(manifest.content_root(), compute_cid(&data));
    }

    #[test]
    fn empty_mime_type_is_the_default() {
        let manifest = FileManifest::for_content(1, b"x", "", vec![]).unwrap();
        assert_eq!(manifest.mime_type.to_vec(), DEFAULT_MIME_TYPE.as_bytes());

        let mut stored = manifest.clone();
        stored.mime_type = BoundedVec::default();
        assert_eq!(stored.mime_type_str(), DEFAULT_MIME_TYPE);
    }

    #[test]
    fn manifest_validate_checks_chunks_against_size() {
        for len in [0, 1, CHUNK_SIZE, CHUNK_SIZE + 1] {
            let data = vec![1u8; len];
            let manifest = FileManifest::for_content(1, &data, DEFAULT_MIME_TYPE, vec![]).unwrap();
            assert_eq!(manifest.validate(), Ok(()));
        }

        let manifest =
            FileManifest::for_content(1, &[1u8; CHUNK_SIZE + 1], DEFAULT_MIME_TYPE, vec![])
                .unwrap();

        let mut wrong_size = manifest.clone();
        wrong_size.total_size = CHUNK_SIZE as u64;
        assert_eq!(wrong_size.validate(), Err(FileSystemError::InvalidManifest));
        wrong_size.total_size = 2 * CHUNK_SIZE as u64 + 1;
        assert_eq!(wrong_size.validate(), Err(FileSystemError::InvalidManifest));
        wrong_size.total_size = u64::MAX;
        assert_eq!(wrong_size.validate(), Err(FileSystemError::InvalidManifest));

        let mut no_chunks = manifest.clone();
        no_chunks.chunks = BoundedVec::default();
        assert_eq!(no_chunks.validate(), Err(FileSystemError::InvalidManifest));

        let mut wrong_sequence = manifest;
        wrong_sequence.chunks[1].sequence = 0;
        assert_eq!(
            wrong_sequence.validate(),
            Err(FileSystemError::InvalidManifest)
        );
    }

    #[test]
    fn manifest_rejects_values_over_bounds() {
        let long_mime = "a".repeat(129);
        assert_eq!(
            FileManifest::for_content(1, b"x", &long_mime, vec![]),
            Err(FileSystemError::MetadataTooLarge)
        );
        let too_many = (0..65)
            .map(|i| MetadataEntry::try_new(format!("k{i}").as_bytes(), b"v").unwrap())
            .collect();
        assert_eq!(
            FileManifest::for_content(1, b"x", DEFAULT_MIME_TYPE, too_many),
            Err(FileSystemError::MetadataTooLarge)
        );
        assert!(MetadataEntry::try_new(&[b'k'; 65], b"v").is_err());
        assert!(MetadataEntry::try_new(b"k", &[b'v'; 257]).is_err());
    }

    #[test]
    fn scale_round_trip() {
        let mut dir = DirectoryNode::new_empty(123);
        dir.upsert_child(entry("file1.txt", EntryType::File, 1024))
            .unwrap();
        let decoded = DirectoryNode::from_blob(&dir.to_blob()).unwrap();
        assert_eq!(decoded, dir);

        let manifest = FileManifest::for_content(
            123,
            b"content",
            "text/plain",
            vec![MetadataEntry::try_new(b"k", b"v").unwrap()],
        )
        .unwrap();
        let decoded = FileManifest::from_blob(&manifest.to_blob()).unwrap();
        assert_eq!(decoded, manifest);
    }

    /// A blob has one encoding, so its CID identifies the node: decoding
    /// fails on trailing bytes and on non-canonical compact lengths.
    #[test]
    fn decode_rejects_trailing_bytes_and_non_canonical_lengths() {
        let mut dir = DirectoryNode::new_empty(123);
        dir.upsert_child(entry("file1.txt", EntryType::File, 1024))
            .unwrap();
        let bytes = dir.to_blob();
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(DirectoryNode::from_blob(&trailing).is_err());
        // The child count 1 in the two-byte compact form: 0x0500, not 0x04.
        let mut non_canonical = bytes[..HEADER_LEN + 8].to_vec();
        non_canonical.extend([0x05, 0x00]);
        non_canonical.extend(&bytes[HEADER_LEN + 9..]);
        assert!(DirectoryNode::from_blob(&non_canonical).is_err());

        let manifest = FileManifest::for_content(123, b"content", "text/plain", vec![]).unwrap();
        let mut trailing = manifest.to_blob();
        trailing.push(0);
        assert!(FileManifest::from_blob(&trailing).is_err());
    }

    #[test]
    fn decode_enforces_bounds() {
        // A name of 257 bytes: compact length 257 = 0x0405, then the bytes.
        let mut bytes = FORMAT_MAGIC.to_vec();
        bytes.push(FORMAT_VERSION);
        bytes.extend(Encode::encode(&7u64));
        bytes.extend(Encode::encode(&codec::Compact(1u32)));
        bytes.extend(Encode::encode(&codec::Compact(257u32)));
        bytes.extend([b'a'; 257]);
        bytes.push(0);
        bytes.extend([0u8; 32 + 8 + 8]);
        bytes.extend(Encode::encode(&codec::Compact(0u32)));
        assert_eq!(
            DirectoryNode::from_blob(&bytes),
            Err(FileSystemError::NotAFormatBlob)
        );
    }

    /// The header tells a newer format apart from data that is not a node at
    /// all.
    #[test]
    fn decode_checks_the_header() {
        let dir = DirectoryNode::new_empty(1).to_blob();
        assert_eq!(&dir[..HEADER_LEN], b"W3FS\x01");
        let mut newer = dir.clone();
        newer[4] = FORMAT_VERSION + 1;
        assert_eq!(
            DirectoryNode::from_blob(&newer),
            Err(FileSystemError::UnsupportedVersion(FORMAT_VERSION + 1))
        );
        assert_eq!(
            DirectoryNode::from_blob(b"raw bucket data"),
            Err(FileSystemError::NotAFormatBlob)
        );
    }

    /// Metadata is stored sorted by key, so one set of entries has one CID,
    /// and a manifest with unsorted or repeated keys is invalid.
    #[test]
    fn manifest_metadata_is_sorted_with_unique_keys() {
        let entry = |k: &[u8]| MetadataEntry::try_new(k, b"v").unwrap();
        let sorted =
            FileManifest::for_content(1, b"x", "", vec![entry(b"b"), entry(b"a")]).unwrap();
        let keys: Vec<&[u8]> = sorted
            .user_metadata
            .iter()
            .map(|e| e.key.as_slice())
            .collect();
        assert_eq!(keys, [&b"a"[..], b"b"]);
        assert_eq!(
            FileManifest::for_content(1, b"x", "", vec![entry(b"a"), entry(b"a")]),
            Err(FileSystemError::DuplicateMetadataKey)
        );
        let mut unsorted = sorted.clone();
        unsorted.user_metadata = vec![entry(b"b"), entry(b"a")].try_into().unwrap();
        assert_eq!(unsorted.validate(), Err(FileSystemError::InvalidManifest));
    }

    #[test]
    fn entry_name_rules() {
        assert!(validate_entry_name(b"a").is_ok());
        assert!(validate_entry_name(&[b'a'; 256]).is_ok());
        assert!(validate_entry_name("ä.txt".as_bytes()).is_ok());
        for bad in [
            &b""[..],
            &[b'a'; 257][..],
            b"a/b",
            b".",
            b"..",
            &[0xff, 0xfe][..],
        ] {
            assert_eq!(
                validate_entry_name(bad),
                Err(FileSystemError::InvalidEntryName)
            );
        }
        assert!(DirectoryEntry::try_new("a/b", EntryType::File, h(b"x"), 0, 0).is_err());
    }

    #[test]
    fn upsert_keeps_children_sorted_and_unique() {
        let mut dir = DirectoryNode::new_empty(1);
        for name in ["b", "a", "c", "B", "aa"] {
            dir.upsert_child(entry(name, EntryType::File, 1)).unwrap();
        }
        let names: Vec<String> = dir.children.iter().map(|e| e.name_str()).collect();
        assert_eq!(names, ["B", "a", "aa", "b", "c"]);

        let mut replacement = entry("aa", EntryType::File, 99);
        replacement.cid = h(b"new");
        dir.upsert_child(replacement).unwrap();
        assert_eq!(dir.children.len(), 5);
        assert_eq!(dir.find_child("aa").unwrap().size, 99);
        assert_eq!(dir.find_child("aa").unwrap().cid, h(b"new"));

        assert!(dir.remove_child("a").is_some());
        assert!(dir.remove_child("a").is_none());
        assert!(dir.find_child("a").is_none());
        dir.validate().unwrap();
    }

    #[test]
    fn upsert_rejects_more_than_max_children() {
        let mut dir = DirectoryNode::new_empty(1);
        for i in 0..MaxDirectoryChildren::get() {
            dir.upsert_child(entry(&format!("{i:05}"), EntryType::File, 0))
                .unwrap();
        }
        assert_eq!(
            dir.upsert_child(entry("overflow", EntryType::File, 0)),
            Err(FileSystemError::DirectoryFull)
        );
    }

    #[test]
    fn validate_rejects_bad_directories() {
        let mut unsorted = DirectoryNode::new_empty(1);
        unsorted.children = BoundedVec::try_from(vec![
            entry("b", EntryType::File, 0),
            entry("a", EntryType::File, 0),
        ])
        .unwrap();
        assert_eq!(unsorted.validate(), Err(FileSystemError::InvalidDirectory));

        let mut duplicate = DirectoryNode::new_empty(1);
        duplicate.children = BoundedVec::try_from(vec![
            entry("a", EntryType::File, 0),
            entry("a", EntryType::Directory, 0),
        ])
        .unwrap();
        assert_eq!(duplicate.validate(), Err(FileSystemError::InvalidDirectory));

        let mut sized_dir = DirectoryNode::new_empty(1);
        sized_dir.children =
            BoundedVec::try_from(vec![entry("d", EntryType::Directory, 5)]).unwrap();
        assert_eq!(sized_dir.validate(), Err(FileSystemError::InvalidDirectory));

        let mut bad_name = DirectoryNode::new_empty(1);
        let mut e = entry("a", EntryType::File, 0);
        e.name = BoundedVec::try_from(b"..".to_vec()).unwrap();
        bad_name.children = BoundedVec::try_from(vec![e]).unwrap();
        assert_eq!(bad_name.validate(), Err(FileSystemError::InvalidEntryName));

        let mut with_metadata = DirectoryNode::new_empty(1);
        with_metadata.metadata =
            BoundedVec::try_from(vec![MetadataEntry::try_new(b"k", b"v").unwrap()]).unwrap();
        assert_eq!(
            with_metadata.validate(),
            Err(FileSystemError::InvalidDirectory)
        );
    }
}
