// SPDX-License-Identifier: Apache-2.0

//! Provider HTTP authentication — the signed `Authorization` header format
//! shared by the client SDK (which builds it) and the provider node (which
//! verifies it).

use crate::error::AuthError;
use storage_primitives::BucketId;

/// Header carrying the block a client acted on, as `<block_number>:<0x block hash>`.
pub const CONTEXT_HEADER: &str = "X-Web3Storage-Context";

/// The block a client acted on, from [`CONTEXT_HEADER`]. The hash is
/// validated but not kept: the provider compares block numbers only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextBlock {
    /// The block number.
    pub number: u32,
}

impl ContextBlock {
    /// Parse a header value; anything but `<number>:<0x + 64 hex digits>` is invalid.
    pub fn parse(value: &str) -> Result<Self, AuthError> {
        let (number, hash) = value
            .split_once(':')
            .ok_or(AuthError::ContextBlockInvalid)?;
        let number = number.parse().map_err(|_| AuthError::ContextBlockInvalid)?;
        hex_array::<32>(hash).ok_or(AuthError::ContextBlockInvalid)?;
        Ok(Self { number })
    }
}

/// Decode `N` bytes of hex, with or without a `0x` prefix.
pub(crate) fn hex_array<const N: usize>(text: &str) -> Option<[u8; N]> {
    let bytes = hex::decode(text.strip_prefix("0x").unwrap_or(text)).ok()?;
    bytes.try_into().ok()
}

/// Build the [`CONTEXT_HEADER`] value for the block a client acted on.
pub fn build_context_header(number: u32, hash: &[u8; 32]) -> String {
    format!("{number}:0x{}", hex::encode(hash))
}

/// The canonical message a client signs for a bucket-scoped request:
/// `web3storage:<METHOD>:<bucket_id>:<timestamp>` (`METHOD` upper-case, `timestamp`
/// Unix seconds). The provider rebuilds this exact string to verify the signature.
pub fn auth_message(method: &str, bucket_id: BucketId, timestamp: &str) -> String {
    format!("web3storage:{method}:{bucket_id}:{timestamp}")
}

/// Build the provider's `Authorization` header value: the sr25519 signature of
/// [`auth_message`] formatted as `Web3Storage <pubkey_hex>:<signature_hex>:<timestamp>`.
/// `sign` returns the 64-byte signature, keeping this keypair-type agnostic.
pub fn build_auth_header(
    pubkey: &[u8; 32],
    method: &str,
    bucket_id: BucketId,
    timestamp: u64,
    sign: impl FnOnce(&[u8]) -> [u8; 64],
) -> String {
    let timestamp = format!("{timestamp}");
    let signature = sign(auth_message(method, bucket_id, &timestamp).as_bytes());
    format!(
        "Web3Storage 0x{}:0x{}:{}",
        hex::encode(pubkey),
        hex::encode(signature),
        timestamp
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_header_round_trips() {
        let value = build_context_header(4213, &[7u8; 32]);
        assert_eq!(
            ContextBlock::parse(&value).unwrap(),
            ContextBlock { number: 4213 }
        );
    }

    #[test]
    fn malformed_context_headers_are_rejected() {
        for value in [
            "",
            "4213",
            "4213:",
            "x:0x00",
            "4213:00",
            "4213:0x1234",
            "-1:0x00",
        ] {
            assert!(
                matches!(
                    ContextBlock::parse(value),
                    Err(AuthError::ContextBlockInvalid)
                ),
                "{value:?} must be rejected"
            );
        }
    }
}
