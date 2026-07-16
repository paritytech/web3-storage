// SPDX-License-Identifier: Apache-2.0

//! Error types for file system operations.

use alloc::string::String;
use codec::{Decode, Encode};
use scale_info::TypeInfo;

/// Error types for file system operations
#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode, TypeInfo)]
#[cfg_attr(feature = "std", derive(thiserror::Error))]
pub enum FileSystemError {
    #[cfg_attr(feature = "std", error("Invalid CID format"))]
    InvalidCid,

    #[cfg_attr(feature = "std", error("Serialization failed"))]
    SerializationError,

    #[cfg_attr(feature = "std", error("Deserialization failed"))]
    DeserializationError,

    #[cfg_attr(feature = "std", error("Entry not found: {0}"))]
    EntryNotFound(String),

    #[cfg_attr(feature = "std", error("Invalid path"))]
    InvalidPath,

    #[cfg_attr(feature = "std", error("Not a directory"))]
    NotADirectory,

    #[cfg_attr(feature = "std", error("Not a file"))]
    NotAFile,
}
