// SPDX-License-Identifier: Apache-2.0

//! CID computation and string conversion helpers.

use alloc::string::String;
use sp_core::H256;

use crate::{Cid, FileSystemError};

/// Compute blake2-256 CID for data
pub fn compute_cid(data: &[u8]) -> Cid {
    sp_core::hashing::blake2_256(data).into()
}

/// Convert CID to hex string (for protobuf storage)
pub fn cid_to_string(cid: &Cid) -> String {
    alloc::format!("0x{}", hex::encode(cid.as_bytes()))
}

/// Parse hex string to CID
pub fn string_to_cid(s: &str) -> Result<Cid, FileSystemError> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    let bytes = hex::decode(s).map_err(|_| FileSystemError::InvalidCid)?;
    if bytes.len() != 32 {
        return Err(FileSystemError::InvalidCid);
    }
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&bytes);
    Ok(H256::from(hash))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_cid() {
        let data = b"hello world";
        let cid = compute_cid(data);
        assert_eq!(cid.as_bytes().len(), 32);
    }

    #[test]
    fn test_cid_string_conversion() {
        let cid = compute_cid(b"test");
        let s = cid_to_string(&cid);
        let decoded = string_to_cid(&s).unwrap();
        assert_eq!(cid, decoded);
    }
}
