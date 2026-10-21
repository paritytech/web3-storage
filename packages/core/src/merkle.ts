// SPDX-License-Identifier: Apache-2.0

// Byte-exact TypeScript port of the provider's drive metadata Merkle root and
// per-file data root, so a client can compute the on-chain integrity anchor
// itself and verify a drive without trusting the provider.
//
// Mirrors, byte for byte:
//   - `crates/providers/storage/src/index/fs.rs`     → `metadata_merkle_root`
//   - `crates/primitives/storage/src/merkle.rs`      → `merkle_root`
//   - `crates/primitives/storage/src/lib.rs`         → `hash_leaf`, `hash_children`, `DEFAULT_CHUNK_SIZE`
//   - `crates/primitives/storage/src/mmr.rs`         → `bag_peaks`, `LEAF_VERSION`, `MmrLeaf::chunk_count`
//
// This is the multi-chunk Merkle DAG walk that `verify.ts` documents as the
// missing "Rust-client parity" piece. Pure functions, no I/O — browser-safe.

import { blake2b256 } from "@polkadot-labs/hdkd-helpers";

import { concatBytes } from "./bytes.js";

/**
 * Mirror of DEFAULT_CHUNK_SIZE in crates/primitives/storage/src/lib.rs: the
 * chunk size of leaf version 1 (see {@link leafChunkSize}).
 */
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
 * Merkle root over `leaves`; an odd last node of a level moves up unchanged.
 * Empty → 32 zero bytes; a single leaf → a copy of that leaf.
 */
export function merkleRoot(leaves: Uint8Array[]): Uint8Array {
  if (leaves.length === 0) return new Uint8Array(32);
  // Copy so the returned root never aliases the caller's input leaf buffer.
  if (leaves.length === 1) return leaves[0].slice();

  let level = leaves.slice();
  while (level.length > 1) {
    const next: Uint8Array[] = [];
    for (let i = 0; i < level.length; i += 2) {
      next.push(i + 1 < level.length ? hashChildren(level[i], level[i + 1]) : level[i]);
    }
    level = next;
  }
  return level[0];
}

/** Mirror of `LEAF_VERSION` in `crates/primitives/storage/src/mmr.rs`. */
export const LEAF_VERSION = 1;

/**
 * Chunk size of MMR leaf version `version`, or undefined for an unknown
 * version. Mirrors `MmrLeaf::chunk_size`.
 */
export function leafChunkSize(version: number): number | undefined {
  switch (version) {
    case 1:
      return DEFAULT_CHUNK_SIZE;
    default:
      return undefined;
  }
}

/**
 * Number of chunks under a committed MMR leaf: every chunk has the chunk size
 * of the leaf's `leaf_version` except the last, and empty data is one chunk.
 * Mirrors `MmrLeaf::chunk_count`. Throws for an unknown version.
 */
export function leafChunkCount(leaf: { leaf_version: number; data_size: number | bigint }): number {
  const chunkSize = leafChunkSize(leaf.leaf_version);
  if (chunkSize === undefined) {
    throw new Error(`unknown MMR leaf version ${leaf.leaf_version}`);
  }
  const size = BigInt(leaf.data_size);
  const count = (size + BigInt(chunkSize) - 1n) / BigInt(chunkSize);
  return Number(count > 0n ? count : 1n);
}

/**
 * A file's `data_root`: chunk the bytes at `DEFAULT_CHUNK_SIZE`, leaf-hash each
 * chunk, then `merkleRoot` over the chunk hashes. An empty file hashes a
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
  return merkleRoot(chunkHashes);
}

/**
 * The drive's metadata Merkle root: one leaf per entry over
 * `utf8(path) ++ data_root[32] ++ u64_le(size)`, entries ordered by UTF-8 byte
 * value (matching Rust's `BTreeMap<String>`), folded by `merkleRoot`.
 * Empty drive → 32 zero bytes.
 */
export function metadataMerkleRoot(entries: MerkleEntry[]): Uint8Array {
  const encoder = new TextEncoder();
  const prepared = entries
    .map((e) => ({ pathBytes: encoder.encode(e.path), dataRoot: e.dataRoot, size: e.size }))
    .sort((a, b) => compareBytes(a.pathBytes, b.pathBytes));

  const leaves = prepared.map((e) => hashLeaf(concatBytes(e.pathBytes, e.dataRoot, u64le(e.size))));
  return merkleRoot(leaves);
}

/** Encode a `u64` as 8 little-endian bytes (`codec::Encode` for a plain `u64`). */
export function u64le(value: bigint): Uint8Array {
  const out = new Uint8Array(8);
  new DataView(out.buffer).setBigUint64(0, value, true);
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
