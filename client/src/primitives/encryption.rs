// SPDX-License-Identifier: Apache-2.0

//! Client-side encryption for data at rest.
//!
//! Encrypts data BEFORE upload so providers store only ciphertext — zero knowledge
//! of contents. Uses XChaCha20-Poly1305 (24-byte nonce, no reuse risk).
//!
//! ## Wire format
//!
//! ```text
//! [1-byte version][24-byte nonce][ciphertext + 16-byte Poly1305 tag]
//! ```
//!
//! - Version `0x01`: XChaCha20-Poly1305 (Rust native)
//! - Version `0x02`: AES-256-GCM (browser WebCrypto, 12-byte IV)

use crate::error::ClientError;
use chacha20poly1305::{
    aead::{Aead, KeyInit, OsRng},
    XChaCha20Poly1305, XNonce,
};
use rand::RngCore;

/// Version byte for XChaCha20-Poly1305 encrypted blobs.
const VERSION_XCHACHA20: u8 = 0x01;

/// XChaCha20-Poly1305 nonce size in bytes.
const NONCE_SIZE: usize = 24;

/// Poly1305 authentication tag size in bytes.
const TAG_SIZE: usize = 16;

/// Overhead added by encryption: 1 (version) + 24 (nonce) + 16 (tag).
pub const ENCRYPTION_OVERHEAD: usize = 1 + NONCE_SIZE + TAG_SIZE;

/// A 256-bit symmetric encryption key.
#[derive(Clone)]
pub struct EncryptionKey([u8; 32]);

impl EncryptionKey {
    /// Create a key from raw bytes.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Generate a random key using a secure RNG.
    pub fn generate() -> Self {
        let mut key = [0u8; 32];
        OsRng.fill_bytes(&mut key);
        Self(key)
    }

    /// Return a reference to the raw key bytes.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl Drop for EncryptionKey {
    fn drop(&mut self) {
        // Zero out key material on drop
        self.0.fill(0);
    }
}

/// Trait for encrypt/decrypt operations.
pub trait Cipher: Send + Sync {
    /// Encrypt plaintext, returning `[version][nonce][ciphertext+tag]`.
    fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>, ClientError>;

    /// Decrypt data produced by [`encrypt`](Cipher::encrypt).
    fn decrypt(&self, data: &[u8]) -> Result<Vec<u8>, ClientError>;
}

/// XChaCha20-Poly1305 cipher (version `0x01`).
pub struct XChaCha20Poly1305Cipher {
    cipher: XChaCha20Poly1305,
}

impl XChaCha20Poly1305Cipher {
    /// Create a new cipher from an [`EncryptionKey`].
    pub fn new(key: &EncryptionKey) -> Self {
        let cipher = XChaCha20Poly1305::new(key.as_bytes().into());
        Self { cipher }
    }
}

impl Cipher for XChaCha20Poly1305Cipher {
    fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>, ClientError> {
        let mut nonce_bytes = [0u8; NONCE_SIZE];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = XNonce::from_slice(&nonce_bytes);

        let ciphertext = self
            .cipher
            .encrypt(nonce, plaintext)
            .map_err(|e| ClientError::Api(format!("Encryption failed: {e}")))?;

        let mut out = Vec::with_capacity(1 + NONCE_SIZE + ciphertext.len());
        out.push(VERSION_XCHACHA20);
        out.extend_from_slice(&nonce_bytes);
        out.extend_from_slice(&ciphertext);
        Ok(out)
    }

    fn decrypt(&self, data: &[u8]) -> Result<Vec<u8>, ClientError> {
        let min_len = 1 + NONCE_SIZE + TAG_SIZE;
        if data.len() < min_len {
            return Err(ClientError::Api(format!(
                "Encrypted data too short: {} bytes, minimum {min_len}",
                data.len()
            )));
        }

        let version = data[0];
        if version != VERSION_XCHACHA20 {
            return Err(ClientError::Api(format!(
                "Unknown encryption version: 0x{version:02x}"
            )));
        }

        let nonce = XNonce::from_slice(&data[1..1 + NONCE_SIZE]);
        let ciphertext = &data[1 + NONCE_SIZE..];

        self.cipher
            .decrypt(nonce, ciphertext)
            .map_err(|_| ClientError::Api("Decryption failed: invalid key or tampered data".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let key = EncryptionKey::generate();
        let cipher = XChaCha20Poly1305Cipher::new(&key);
        let plaintext = b"Hello, encrypted world!";

        let encrypted = cipher.encrypt(plaintext).unwrap();
        assert_ne!(&encrypted[..], plaintext);
        assert_eq!(encrypted.len(), plaintext.len() + ENCRYPTION_OVERHEAD);

        let decrypted = cipher.decrypt(&encrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn wrong_key_fails() {
        let key1 = EncryptionKey::generate();
        let key2 = EncryptionKey::generate();
        let cipher1 = XChaCha20Poly1305Cipher::new(&key1);
        let cipher2 = XChaCha20Poly1305Cipher::new(&key2);

        let encrypted = cipher1.encrypt(b"secret").unwrap();
        assert!(cipher2.decrypt(&encrypted).is_err());
    }

    #[test]
    fn tampered_data_fails() {
        let key = EncryptionKey::generate();
        let cipher = XChaCha20Poly1305Cipher::new(&key);

        let mut encrypted = cipher.encrypt(b"tamper test").unwrap();
        // Flip a byte in the ciphertext portion
        let last = encrypted.len() - 1;
        encrypted[last] ^= 0xFF;
        assert!(cipher.decrypt(&encrypted).is_err());
    }

    #[test]
    fn short_ciphertext_fails() {
        let key = EncryptionKey::generate();
        let cipher = XChaCha20Poly1305Cipher::new(&key);

        // Too short to contain version + nonce + tag
        let short = vec![0x01; 10];
        assert!(cipher.decrypt(&short).is_err());
    }

    #[test]
    fn unknown_version_fails() {
        let key = EncryptionKey::generate();
        let cipher = XChaCha20Poly1305Cipher::new(&key);

        let mut encrypted = cipher.encrypt(b"version test").unwrap();
        encrypted[0] = 0xFF; // invalid version
        let err = cipher.decrypt(&encrypted).unwrap_err();
        assert!(err.to_string().contains("Unknown encryption version"));
    }

    #[test]
    fn empty_plaintext() {
        let key = EncryptionKey::generate();
        let cipher = XChaCha20Poly1305Cipher::new(&key);

        let encrypted = cipher.encrypt(b"").unwrap();
        assert_eq!(encrypted.len(), ENCRYPTION_OVERHEAD);

        let decrypted = cipher.decrypt(&encrypted).unwrap();
        assert!(decrypted.is_empty());
    }

    #[test]
    fn large_plaintext() {
        let key = EncryptionKey::generate();
        let cipher = XChaCha20Poly1305Cipher::new(&key);

        let plaintext = vec![0xAB; 1024 * 1024]; // 1 MiB
        let encrypted = cipher.encrypt(&plaintext).unwrap();
        assert_eq!(encrypted.len(), plaintext.len() + ENCRYPTION_OVERHEAD);

        let decrypted = cipher.decrypt(&encrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn version_byte_is_correct() {
        let key = EncryptionKey::generate();
        let cipher = XChaCha20Poly1305Cipher::new(&key);

        let encrypted = cipher.encrypt(b"check version").unwrap();
        assert_eq!(encrypted[0], 0x01);
    }

    #[test]
    fn different_encryptions_produce_different_ciphertexts() {
        let key = EncryptionKey::generate();
        let cipher = XChaCha20Poly1305Cipher::new(&key);
        let plaintext = b"nonce uniqueness";

        let enc1 = cipher.encrypt(plaintext).unwrap();
        let enc2 = cipher.encrypt(plaintext).unwrap();
        // Different random nonces should produce different ciphertexts
        assert_ne!(enc1, enc2);
        // But both decrypt to the same plaintext
        assert_eq!(cipher.decrypt(&enc1).unwrap(), plaintext);
        assert_eq!(cipher.decrypt(&enc2).unwrap(), plaintext);
    }
}
