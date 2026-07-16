// SPDX-License-Identifier: Apache-2.0

//! Bound marker types for the SCALE-encoded file system structures.

use sp_runtime::traits::Get;

/// Maximum length for entry names (256 bytes)
pub struct MaxEntryNameLength;
impl Get<u32> for MaxEntryNameLength {
    fn get() -> u32 {
        256
    }
}

/// Maximum length for CID strings (66 bytes for "0x" + 64 hex chars)
pub struct MaxCidStringLength;
impl Get<u32> for MaxCidStringLength {
    fn get() -> u32 {
        66
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
