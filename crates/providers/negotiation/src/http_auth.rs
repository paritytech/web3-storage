// SPDX-License-Identifier: Apache-2.0

//! Provider HTTP authentication — the signed `Authorization` header format
//! shared by the client SDK (which builds it) and the provider node (which
//! verifies it).

use storage_primitives::BucketId;

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
