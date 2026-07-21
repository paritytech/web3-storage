# Client-Side Encryption Design

## Overview

Client-side encryption (A1) ensures data is encrypted **before** it leaves the client. Storage providers store only ciphertext and have zero knowledge of file contents. This is a core Web3 principle: user data sovereignty.

### Design principles

1. **Encrypt-before-upload**: The entire blob is encrypted client-side, then sent to the provider. The provider chunks, hashes, and builds Merkle trees over the ciphertext.
2. **Provider requires zero changes**: Encryption is purely a client concern. The provider stores opaque bytes.
3. **Opt-in**: Encryption is off by default. Users enable it explicitly with a key.
4. **Self-contained wire format**: Each encrypted blob starts with a version byte, so the cipher is unambiguous at decryption time.

## Wire Format

```
[1-byte version][nonce/IV][ciphertext + authentication tag]
```

| Version | Cipher | Nonce | Tag | Environment |
|---------|--------|-------|-----|-------------|
| `0x01` | XChaCha20-Poly1305 | 24 bytes | 16 bytes | Rust native |
| `0x02` | AES-256-GCM | 12 bytes | 16 bytes | Browser (WebCrypto) |

The version byte enables forward compatibility: future ciphers get new version numbers, and old data can always be decrypted if you know the key and have the matching implementation.

## Cipher Selection

### Rust: XChaCha20-Poly1305 (version `0x01`)

**Why XChaCha20-Poly1305 over other options?**

| Cipher | Nonce size | Nonce reuse risk | Hardware accel | Key size | AEAD |
|--------|-----------|------------------|----------------|----------|------|
| **XChaCha20-Poly1305** | 24 bytes | Negligible | Software-fast | 256-bit | Yes |
| AES-256-GCM | 12 bytes | Dangerous | AES-NI required | 256-bit | Yes |
| ChaCha20-Poly1305 | 12 bytes | Moderate risk | Software-fast | 256-bit | Yes |
| AES-256-CBC + HMAC | 16 bytes | Low | AES-NI required | 256-bit | Manual |

#### Why not AES-256-GCM in Rust?

AES-GCM is the industry standard and widely deployed. However, for our use case in Rust:

1. **Nonce reuse is catastrophic**: AES-GCM with a 12-byte (96-bit) nonce has a birthday bound collision probability that becomes non-negligible after ~2^32 encryptions with the same key. If two messages share a nonce under the same key, the authentication is completely broken and plaintext can be recovered. In a decentralized storage system where the same key may be used across multiple devices and long time horizons, this is a real risk.

2. **Hardware dependency**: AES-GCM performance depends heavily on AES-NI hardware instructions. Without them (ARM devices, some cloud VMs), it's significantly slower and vulnerable to timing side-channels. XChaCha20 is constant-time in software on all platforms.

3. **XChaCha20-Poly1305's 24-byte nonce eliminates reuse risk**: With a 192-bit nonce space, randomly generating nonces is safe for virtually unlimited encryptions (~2^96 messages before birthday bound). No counter management needed, no coordination between devices.

#### Why not plain ChaCha20-Poly1305?

ChaCha20-Poly1305 (RFC 8439) uses a 12-byte nonce, which has the same birthday-bound concerns as AES-GCM. The "X" variant (XChaCha20) extends the nonce to 24 bytes via HChaCha20, eliminating this concern at negligible performance cost.

#### Why not AES-256-CBC + HMAC-SHA256?

This is the "encrypt-then-MAC" composition. While secure when implemented correctly:

1. **Two primitives to get right**: Must encrypt-then-MAC (not MAC-then-encrypt or encrypt-and-MAC). The wrong order leads to padding oracle attacks.
2. **More code, more bugs**: AEAD ciphers like XChaCha20-Poly1305 provide authenticated encryption in a single operation, eliminating the possibility of miscomposing encrypt and MAC.
3. **No benefit**: XChaCha20-Poly1305 is strictly better — faster, simpler, and equally secure.

#### Why not AES-256-SIV (AES-SIV)?

AES-SIV is nonce-misuse-resistant (nonce reuse only leaks whether two plaintexts are identical, not the plaintext itself). This is excellent, but:

1. **Two-pass**: SIV requires two passes over the data (one for the MAC, one for encryption), which doubles throughput cost for large blobs.
2. **Limited ecosystem**: Not available in WebCrypto, which would prevent the browser implementation from being interoperable.
3. **Overkill**: XChaCha20-Poly1305's 24-byte random nonce already makes nonce collision practically impossible.

### Browser: AES-256-GCM (version `0x02`)

**Why AES-GCM in the browser but not in Rust?**

1. **WebCrypto only offers AES-GCM**: The Web Cryptography API does not support ChaCha20 or XChaCha20. AES-GCM is the only AEAD cipher available.
2. **Hardware-accelerated in browsers**: All modern browsers use the platform's AES-NI (or ARM AES extensions), making AES-GCM fast and constant-time.
3. **Nonce reuse risk is lower in browser context**: A browser session is short-lived and single-device. The 12-byte nonce with random generation is acceptable for the typical number of encryptions in a browser session.

### Cross-environment compatibility

The version byte (`0x01` vs `0x02`) means data encrypted in Rust can be decrypted in Rust, and data encrypted in the browser can be decrypted in the browser. Cross-environment decryption (e.g., Rust decrypting browser-encrypted data) is a future extension — it would require adding an AES-GCM implementation to Rust and a ChaCha20 polyfill to the browser.

For V1, this is acceptable: users typically upload from one environment and download from the same environment.

## Key Management

### V1: Raw symmetric key

The user provides a 256-bit (32-byte) symmetric key. This is the simplest possible approach:

```rust
// Rust
let key = EncryptionKey::generate();
let client = StorageUserClient::with_defaults()?.with_encryption_key(&key);
```

```typescript
// TypeScript
const { encryptionKey, rawKey } = await EncryptionKey.generate();
client.setEncryptionKey(encryptionKey);
// User must save rawKey (displayed as hex) securely
```

**What's explicitly out of scope for V1:**

- **Password-based key derivation (KDF)**: No Argon2/scrypt/PBKDF2. Users who want password-derived keys can do this externally and pass the derived key.
- **Key wrapping / envelope encryption**: No key hierarchy. One key encrypts/decrypts everything.
- **Key exchange**: No Diffie-Hellman or asymmetric encryption for key sharing between users.
- **Key storage**: The client does not persist keys. The user is responsible for saving and managing their key.

These are all planned for future iterations.

### Security implications

- **Lost key = lost data**: There is no recovery mechanism. This is intentional — the provider cannot help because it never sees the key.
- **Key reuse across blobs**: The same key encrypts all blobs for a given client session. This is safe because each encryption generates a fresh random nonce.
- **No key rotation**: Changing keys requires re-uploading all data. This is acceptable for V1.

## Architecture

### Data flow with encryption

```
Upload:
  plaintext → encrypt(key) → [version][nonce][ciphertext+tag] → provider.upload()
                                                                      ↓
                                                          provider chunks & hashes
                                                          provider builds Merkle tree
                                                          provider commits to MMR

Download:
  provider.download() → [version][nonce][ciphertext+tag] → decrypt(key) → plaintext
```

### Impact on existing features

| Feature | Impact | Notes |
|---------|--------|-------|
| Chunking | None | Provider chunks the ciphertext |
| Merkle proofs | None | Proofs are over ciphertext chunks |
| MMR commitments | None | MMR commits ciphertext hashes |
| Challenges | None | Challenges verify ciphertext integrity |
| Spot checks | None | Verify ciphertext chunks, not plaintext |
| Range reads | **Breaks** | Cannot decrypt partial ciphertext |

**Range reads**: With encryption, the full blob must be downloaded and decrypted. Partial range reads of encrypted data are meaningless because the authentication tag covers the entire blob. This is a known V1 limitation.

## Performance

### Overhead

| Component | Overhead |
|-----------|----------|
| Encryption (XChaCha20) | ~1 GB/s on modern hardware |
| Encryption (AES-GCM) | ~2-5 GB/s with AES-NI |
| Size overhead (Rust) | 41 bytes per blob (1 + 24 + 16) |
| Size overhead (Browser) | 29 bytes per blob (1 + 12 + 16) |

The overhead is negligible for any realistic file size. A 1 MiB file grows by 0.004%.

### Memory

Encryption is done in-memory before upload. This means the entire plaintext and the entire ciphertext must fit in memory simultaneously. For very large files (multi-GB), streaming encryption would be needed — this is a future optimization.

## Future Extensions

1. **Streaming encryption**: Use chunked AEAD (e.g., STREAM construction from libsodium) to encrypt large files without holding entire blob in memory.
2. **Key derivation**: Add Argon2id KDF for password-based encryption.
3. **Per-file keys**: Generate a random data encryption key (DEK) per file, wrap it with a key encryption key (KEK). Enables key rotation without re-uploading.
4. **Shared encryption**: Use asymmetric encryption (X25519) to share DEKs between users.
5. **Cross-environment decryption**: Add AES-GCM support to Rust client and XChaCha20 polyfill to browser.
6. **Encrypted metadata**: Encrypt file names and directory structures, not just content.
7. **File System (Layer 1) integration**: Wire encryption into the file system client.

## References

- [XChaCha20-Poly1305 (IETF draft)](https://datatracker.ietf.org/doc/html/draft-irtf-cfrg-xchacha-03)
- [ChaCha20-Poly1305 (RFC 8439)](https://datatracker.ietf.org/doc/html/rfc8439)
- [AES-GCM (NIST SP 800-38D)](https://csrc.nist.gov/publications/detail/sp/800-38d/final)
- [Web Cryptography API](https://www.w3.org/TR/WebCryptoAPI/)
- [Nonce misuse resistance (Rogaway & Shrimpton)](https://cseweb.ucsd.edu/~mihir/papers/oem.pdf)
- [libsodium STREAM construction](https://doc.libsodium.org/secret-key_cryptography/secretstream)
