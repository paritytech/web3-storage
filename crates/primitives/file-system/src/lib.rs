// SPDX-License-Identifier: Apache-2.0

//! File System Primitives for Layer 1
//!
//! This crate provides the core data structures for the Layer 1 file system
//! built on top of Layer 0 (Scalable Web3 Storage).
//!
//! # Architecture
//!
//! - **Layer 0**: Raw blob storage in buckets (content-addressed chunks)
//! - **Layer 1**: File system metadata (directories, file manifests)
//! - **Layer 2**: User interfaces (FUSE, web UI, CLI)
//!
//! # Key Concepts
//!
//! - **Drive**: A user's logical file system, mapped to a Layer 0 bucket
//! - **RootCID**: The content ID of the root directory, stored on-chain
//! - **DirectoryNode**: A directory containing references to children
//! - **FileManifest**: Metadata about a file and its chunks
//! - **CID**: Content Identifier (blake2-256 hash)
//!
//! # Type System
//!
//! This crate provides two sets of types:
//! - **SCALE types** (always available): Used for on-chain storage, `no_std` compatible
//! - **Proto types** (std only): Used for off-chain serialization via protobuf

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

#[cfg(feature = "std")]
extern crate std;

pub mod bounds;
pub mod cid;
#[cfg(feature = "std")]
mod convert;
pub mod directory;
pub mod drive;
pub mod error;
pub mod manifest;
/// Protobuf-generated types for off-chain serialization (std only)
#[cfg(feature = "std")]
pub mod proto;

pub use bounds::*;
pub use cid::*;
pub use directory::*;
pub use drive::*;
pub use error::*;
pub use manifest::*;

/// Drive identifier (unique ID for each drive)
pub type DriveId = u64;

/// Agreement identifier from Layer 0
pub type AgreementId = u64;

/// Content Identifier (blake2-256 hash)
pub type Cid = sp_core::H256;
