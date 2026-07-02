// SPDX-License-Identifier: Apache-2.0

/**
 * Client-side encryption using WebCrypto AES-256-GCM.
 *
 * Wire format: [0x02][12-byte IV][ciphertext + 16-byte GCM tag]
 *
 * Version bytes:
 *   0x01 = XChaCha20-Poly1305 (Rust native)
 *   0x02 = AES-256-GCM (browser WebCrypto)
 */

const VERSION_AES_GCM = 0x02;
const IV_SIZE = 12;
const TAG_SIZE = 16;

/** Overhead added by encryption: 1 (version) + 12 (IV) + 16 (tag). */
export const ENCRYPTION_OVERHEAD = 1 + IV_SIZE + TAG_SIZE;

/**
 * Encryption key wrapping a WebCrypto CryptoKey for AES-256-GCM.
 */
export class EncryptionKey {
  private cryptoKey: CryptoKey;

  private constructor(cryptoKey: CryptoKey) {
    this.cryptoKey = cryptoKey;
  }

  /** Import a raw 32-byte key. */
  static async fromBytes(rawKey: Uint8Array): Promise<EncryptionKey> {
    if (rawKey.length !== 32) {
      throw new Error(`Encryption key must be 32 bytes, got ${rawKey.length}`);
    }
    const cryptoKey = await crypto.subtle.importKey(
      "raw",
      // TS 6.0 types `Uint8Array` as `Uint8Array<ArrayBufferLike>`, but Web Crypto
      // wants an `ArrayBuffer`-backed view; browser buffers are never shared.
      rawKey as Uint8Array<ArrayBuffer>,
      { name: "AES-GCM", length: 256 },
      false,
      ["encrypt", "decrypt"],
    );
    return new EncryptionKey(cryptoKey);
  }

  /** Generate a random 256-bit key. Returns the key and its raw bytes. */
  static async generate(): Promise<{ encryptionKey: EncryptionKey; rawKey: Uint8Array }> {
    const rawKey = new Uint8Array(32);
    crypto.getRandomValues(rawKey);
    const encryptionKey = await EncryptionKey.fromBytes(rawKey);
    return { encryptionKey, rawKey };
  }

  /** Encrypt plaintext into `[0x02][IV][ciphertext+tag]`. */
  async encrypt(plaintext: Uint8Array): Promise<Uint8Array> {
    const iv = new Uint8Array(IV_SIZE);
    crypto.getRandomValues(iv);

    const ciphertext = await crypto.subtle.encrypt(
      { name: "AES-GCM", iv },
      this.cryptoKey,
      plaintext as Uint8Array<ArrayBuffer>,
    );

    const out = new Uint8Array(1 + IV_SIZE + ciphertext.byteLength);
    out[0] = VERSION_AES_GCM;
    out.set(iv, 1);
    out.set(new Uint8Array(ciphertext), 1 + IV_SIZE);
    return out;
  }

  /** Decrypt data produced by `encrypt()`. */
  async decrypt(data: Uint8Array): Promise<Uint8Array> {
    const minLen = 1 + IV_SIZE + TAG_SIZE;
    if (data.length < minLen) {
      throw new Error(
        `Encrypted data too short: ${data.length} bytes, minimum ${minLen}`,
      );
    }

    const version = data[0]!;
    if (version !== VERSION_AES_GCM) {
      throw new Error(
        `Unknown encryption version: 0x${version.toString(16).padStart(2, "0")}`,
      );
    }

    const iv = data.slice(1, 1 + IV_SIZE);
    const ciphertext = data.slice(1 + IV_SIZE);

    const plaintext = await crypto.subtle.decrypt(
      { name: "AES-GCM", iv },
      this.cryptoKey,
      ciphertext,
    );

    return new Uint8Array(plaintext);
  }
}

/** Parse a hex string (with or without 0x prefix) into bytes. */
export function hexToBytes(hex: string): Uint8Array {
  const clean = hex.startsWith("0x") ? hex.slice(2) : hex;
  if (clean.length % 2 !== 0) {
    throw new Error("Invalid hex string length");
  }
  const bytes = new Uint8Array(clean.length / 2);
  for (let i = 0; i < bytes.length; i++) {
    bytes[i] = parseInt(clean.slice(i * 2, i * 2 + 2), 16);
  }
  return bytes;
}

/** Encode bytes as a hex string (no 0x prefix). */
export function bytesToHex(bytes: Uint8Array): string {
  return Array.from(bytes)
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
}
