// SPDX-License-Identifier: GPL-3.0-only
//
// Byte-exact TypeScript port of the provider's drive metadata Merkle root and
// per-file data root, so a client can compute the on-chain integrity anchor
// itself and verify a drive without trusting the provider.
//
// Mirrors, byte for byte:
//   - `provider-node/src/fs_index.rs`     → `metadata_merkle_root`
//   - `provider-node/src/storage/mod.rs`  → `build_padded_merkle_tree`
//   - `primitives/src/lib.rs`             → `blake2_256`, `hash_children`, `DEFAULT_CHUNK_SIZE`
//
// This is the multi-chunk Merkle DAG walk that `verify.ts` documents as the
// missing "Rust-client parity" piece. Pure functions, no I/O — browser-safe.

import { computeCid, DEFAULT_CHUNK_SIZE } from "./verify.js";

/** One drive entry as it contributes to the metadata Merkle tree. */
export interface MerkleEntry {
  /** Absolute path, exactly as the provider keys it (e.g. `/Beach/photo.jpg`). */
  path: string;
  /** 32-byte content root (zero-filled for directories). */
  dataRoot: Uint8Array;
  /** Original byte size (0 for directories). */
  size: bigint;
}

/** Hash an internal node: `blake2_256(left[32] ++ right[32])`. */
export function hashChildren(left: Uint8Array, right: Uint8Array): Uint8Array {
  return computeCid(concatBytes(left, right));
}

/**
 * Balanced Merkle root over `leaves`, padded to the next power of two with
 * 32-byte zero hashes. Empty → 32 zero bytes; a single leaf → that leaf verbatim.
 * Identical to the provider's `build_padded_merkle_tree` / the tail of
 * `metadata_merkle_root`.
 */
export function paddedMerkleRoot(leaves: Uint8Array[]): Uint8Array {
  if (leaves.length === 0) return new Uint8Array(32);
  if (leaves.length === 1) return leaves[0];

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
 * A file's `data_root`: chunk the bytes at `DEFAULT_CHUNK_SIZE`, blake2-256 each
 * chunk, then `paddedMerkleRoot` over the chunk hashes. An empty file hashes a
 * single empty chunk; a single chunk yields its own hash. Mirrors `fs_put_file`.
 */
export function computeDataRoot(bytes: Uint8Array): Uint8Array {
  const chunkHashes: Uint8Array[] = [];
  if (bytes.length === 0) {
    chunkHashes.push(computeCid(new Uint8Array(0)));
  } else {
    for (let off = 0; off < bytes.length; off += DEFAULT_CHUNK_SIZE) {
      chunkHashes.push(computeCid(bytes.subarray(off, Math.min(off + DEFAULT_CHUNK_SIZE, bytes.length))));
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

  const leaves = prepared.map((e) => computeCid(concatBytes(e.pathBytes, e.dataRoot, u64le(e.size))));
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

function concatBytes(...arrays: Uint8Array[]): Uint8Array {
  let total = 0;
  for (const a of arrays) total += a.length;
  const out = new Uint8Array(total);
  let off = 0;
  for (const a of arrays) {
    out.set(a, off);
    off += a.length;
  }
  return out;
}

/** Lexicographic comparison of two byte arrays (Rust `[u8]`/`str` ordering). */
function compareBytes(a: Uint8Array, b: Uint8Array): number {
  const len = Math.min(a.length, b.length);
  for (let i = 0; i < len; i++) {
    if (a[i] !== b[i]) return a[i] - b[i];
  }
  return a.length - b.length;
}
