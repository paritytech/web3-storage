// SPDX-License-Identifier: Apache-2.0

// Byte-exact TypeScript port of the provider's drive metadata Merkle root and
// per-file data root, so a client can compute the on-chain integrity anchor
// itself and verify a drive without trusting the provider.
//
// This is the multi-chunk Merkle DAG walk that `verify.ts` documents as the
// missing "Rust-client parity" piece. Pure functions, no I/O — browser-safe.

import { blake2b256 } from "@polkadot-labs/hdkd-helpers";

import { concatBytes } from "./bytes.js";

/** Mirror of DEFAULT_CHUNK_SIZE in crates/primitives/storage/src/lib.rs. */
export const DEFAULT_CHUNK_SIZE = 256 * 1024;

/** Prefix of an internal node preimage; leaves use a different prefix. */
const NODE_PREFIX = 0x01;

/** Prefix of a Merkle leaf preimage; internal nodes use a different prefix. */
const LEAF_PREFIX = 0x00;

/** Prefix of an MMR peak-bagging preimage. */
const PEAK_PREFIX = 0x02;

/** One drive entry as it contributes to the metadata Merkle tree. */
export interface MerkleEntry {
  /** Absolute path, exactly as the provider keys it (e.g. `/Beach/photo.jpg`). */
  path: string;
  /** 32-byte content root (zero-filled for directories). */
  dataRoot: Uint8Array;
  /** Original byte size (0 for directories). */
  size: bigint;
}

/** Hash an internal node: `blake2_256(0x01 ++ left[32] ++ right[32])`. */
export function hashChildren(left: Uint8Array, right: Uint8Array): Uint8Array {
  return blake2b256(concatBytes(Uint8Array.of(NODE_PREFIX), left, right));
}

/**
 * Hash a Merkle leaf: `blake2b-256(0x00 ++ data)`.
 * The prefix keeps the bytes of an internal node
 * from hashing to the same value as a leaf.
 */
export function hashLeaf(data: Uint8Array): Uint8Array {
  const preimage = new Uint8Array(1 + data.length);
  preimage[0] = LEAF_PREFIX;
  preimage.set(data, 1);
  return blake2b256(preimage);
}

/**
 * Combine MMR peaks into the MMR root. Peaks fold from right to left with
 * `blake2b-256(0x02 ++ peak ++ rest)`; one peak is the root itself and no
 * peaks give 32 zero bytes.
 */
export function bagPeaks(peaks: Uint8Array[]): Uint8Array {
  if (peaks.length === 0) return new Uint8Array(32);
  return peaks.reduceRight((rest, peak) => blake2b256(concatBytes(Uint8Array.of(PEAK_PREFIX), peak, rest)));
}

/**
 * Balanced Merkle root over `leaves`, padded to the next power of two with
 * 32-byte zero hashes. Empty → 32 zero bytes; a single leaf → that leaf verbatim.
 * Identical to the provider's `build_padded_merkle_tree` / the tail of
 * `metadata_merkle_root`.
 */
export function paddedMerkleRoot(leaves: Uint8Array[]): Uint8Array {
  if (leaves.length === 0) return new Uint8Array(32);
  // Copy so the returned root never aliases the caller's input leaf buffer.
  if (leaves.length === 1) return leaves[0].slice();

  let level = leaves.slice();
  const paddedLen = nextPowerOfTwo(level.length);
  while (level.length < paddedLen) level.push(new Uint8Array(32));

  while (level.length > 1) {
    const next: Uint8Array[] = [];
    for (let i = 0; i < level.length; i += 2) {
      next.push(hashChildren(level[i], level[i + 1]));
    }
    level = next;
  }
  return level[0];
}

/**
 * A file's `data_root`: chunk the bytes at `DEFAULT_CHUNK_SIZE`, leaf-hash each
 * chunk, then `paddedMerkleRoot` over the chunk hashes. An empty file hashes a
 * single empty chunk; a single chunk yields its own hash. Mirrors `fs_put_file`.
 */
export function computeDataRoot(bytes: Uint8Array): Uint8Array {
  const chunkHashes: Uint8Array[] = [];
  if (bytes.length === 0) {
    chunkHashes.push(hashLeaf(new Uint8Array(0)));
  } else {
    for (let off = 0; off < bytes.length; off += DEFAULT_CHUNK_SIZE) {
      chunkHashes.push(hashLeaf(bytes.subarray(off, Math.min(off + DEFAULT_CHUNK_SIZE, bytes.length))));
    }
  }
  return paddedMerkleRoot(chunkHashes);
}

/**
 * The drive's metadata Merkle root: one leaf per entry over
 * `utf8(path) ++ data_root[32] ++ u64_le(size)`, entries ordered by UTF-8 byte
 * value (matching Rust's `BTreeMap<String>`), folded by `paddedMerkleRoot`.
 * Empty drive → 32 zero bytes.
 */
export function metadataMerkleRoot(entries: MerkleEntry[]): Uint8Array {
  const encoder = new TextEncoder();
  const prepared = entries
    .map((e) => ({ pathBytes: encoder.encode(e.path), dataRoot: e.dataRoot, size: e.size }))
    .sort((a, b) => compareBytes(a.pathBytes, b.pathBytes));

  const leaves = prepared.map((e) => hashLeaf(concatBytes(e.pathBytes, e.dataRoot, u64le(e.size))));
  return paddedMerkleRoot(leaves);
}

/** Encode a `u64` as 8 little-endian bytes (`codec::Encode` for a plain `u64`). */
export function u64le(value: bigint): Uint8Array {
  const out = new Uint8Array(8);
  new DataView(out.buffer).setBigUint64(0, value, true);
  return out;
}

function nextPowerOfTwo(n: number): number {
  let p = 1;
  while (p < n) p <<= 1;
  return p;
}

/** Lexicographic comparison of two byte arrays (Rust `[u8]`/`str` ordering). */
function compareBytes(a: Uint8Array, b: Uint8Array): number {
  const len = Math.min(a.length, b.length);
  for (let i = 0; i < len; i++) {
    if (a[i] !== b[i]) return a[i] - b[i];
  }
  return a.length - b.length;
}
