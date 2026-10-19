// SPDX-License-Identifier: Apache-2.0

// TypeScript port of the Merkle functions the provider and the primitives
// use, so a client can compute and check CIDs itself without trusting the
// provider.
//
// Mirrors, byte for byte:
//   - `crates/providers/storage/src/index/fs.rs`     → `metadata_merkle_root`
//   - `crates/primitives/storage/src/merkle.rs`      → `merkle_root`, `merkle_tree`, `chunk_count`,
//                                                       `verify_merkle_proof`
//   - `crates/primitives/storage/src/lib.rs`         → `hash_leaf`, `hash_children`, `bag_peaks`,
//                                                       `verify_mmr_proof`, `DEFAULT_CHUNK_SIZE`
//
// Pure functions, no I/O, browser-safe.

import { blake2b256 } from "@polkadot-labs/hdkd-helpers";

import { bytesEq, concatBytes } from "./bytes.js";

/**
 * Mirror of DEFAULT_CHUNK_SIZE in crates/primitives/storage/src/lib.rs: the
 * chunk size, and the largest leaf a challenge response can contain.
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
  return merkleTree(leaves).root;
}

/** Internal node of a Merkle tree: `hash = hashChildren(left, right)`. */
export interface MerkleNode {
  hash: Uint8Array;
  left: Uint8Array;
  right: Uint8Array;
}

/**
 * Root and internal nodes of the Merkle tree over `leaves`, mirroring
 * `storage_primitives::merkle_tree`. `nodes` is bottom-up, level by level,
 * left to right; a provider stores each one with data `left ++ right` and
 * children `[left, right]`. No leaves → zero root; one leaf → a copy of that
 * leaf, no nodes. Equal subtrees appear once per position, so a hash can repeat.
 */
export function merkleTree(leaves: Uint8Array[]): { root: Uint8Array; nodes: MerkleNode[] } {
  if (leaves.length === 0) return { root: new Uint8Array(32), nodes: [] };
  // Copy so the returned root never aliases the caller's input leaf buffer.
  if (leaves.length === 1) return { root: leaves[0].slice(), nodes: [] };

  let level = leaves.slice();
  const nodes: MerkleNode[] = [];
  while (level.length > 1) {
    const next: Uint8Array[] = [];
    for (let i = 0; i < level.length; i += 2) {
      // An odd last node moves up unchanged.
      if (i + 1 === level.length) {
        next.push(level[i]);
        continue;
      }
      const hash = hashChildren(level[i], level[i + 1]);
      nodes.push({ hash, left: level[i], right: level[i + 1] });
      next.push(hash);
    }
    level = next;
  }
  return { root: level[0], nodes };
}

/**
 * Split bytes into `DEFAULT_CHUNK_SIZE` chunks. Empty input is one empty
 * chunk. The chunks are views into `bytes`.
 */
export function splitChunks(bytes: Uint8Array): Uint8Array[] {
  if (bytes.length === 0) return [new Uint8Array(0)];
  const chunks: Uint8Array[] = [];
  for (let off = 0; off < bytes.length; off += DEFAULT_CHUNK_SIZE) {
    chunks.push(bytes.subarray(off, Math.min(off + DEFAULT_CHUNK_SIZE, bytes.length)));
  }
  return chunks;
}

/**
 * Number of chunks (tree leaves) of data with `dataSize` bytes; empty data is
 * one chunk. Mirrors `chunk_count` in `crates/primitives/storage/src/merkle.rs`.
 */
export function chunkCount(dataSize: bigint | number): number {
  return Math.max(1, Math.ceil(Number(dataSize) / DEFAULT_CHUNK_SIZE));
}

/**
 * A blob's `data_root`, which is also its CID: chunk the bytes at
 * `DEFAULT_CHUNK_SIZE`, leaf-hash each chunk, then `merkleRoot` over the chunk
 * hashes. An empty blob hashes a single empty chunk; a single chunk yields its
 * own hash. Mirrors `fs_put_file`.
 */
export function computeDataRoot(bytes: Uint8Array): Uint8Array {
  return merkleRoot(splitChunks(bytes).map(hashLeaf));
}

/**
 * Check a chunk proof from `GET /read` (the siblings of
 * `storage_primitives::build_merkle_proofs`) against `dataRoot`, like
 * `storage_primitives::verify_merkle_proof`. The path bits come from `index`.
 * `chunks` is the blob's chunk count; it fixes the expected sibling count, so
 * a proof for a different tree shape fails.
 */
export function verifyChunkProof(
  chunkHash: Uint8Array,
  index: number,
  siblings: Uint8Array[],
  dataRoot: Uint8Array,
  chunks: number,
): boolean {
  if (index < 0 || index >= chunks) return false;
  let current = chunkHash;
  let idx = index;
  let width = chunks;
  let used = 0;
  while (width > 1) {
    // An odd last node moves up without a sibling.
    if (idx !== width - 1 || width % 2 === 0) {
      if (used === siblings.length) return false;
      const sibling = siblings[used++];
      current = idx % 2 === 1 ? hashChildren(sibling, current) : hashChildren(current, sibling);
    }
    idx = Math.floor(idx / 2);
    width = Math.ceil(width / 2);
  }
  return used === siblings.length && bytesEq(current, dataRoot);
}

/** `MmrLeaf` of `storage_primitives`. */
export interface MmrLeaf {
  dataRoot: Uint8Array;
  dataSize: bigint;
  totalSize: bigint;
}

/** `MmrProof` of `storage_primitives`, with the leaf proof flattened. */
export interface MmrProof {
  peaks: Uint8Array[];
  leaf: MmrLeaf;
  siblings: Uint8Array[];
  /** Path bits, leaf to peak: `true` = the current node is the right child. */
  path: boolean[];
}

/**
 * {@link verifyMmrProof} for the last leaf of an MMR with `leafCount` leaves.
 * A proof for an older leaf fails.
 */
export function verifyLastMmrLeaf(proof: MmrProof, mmrRoot: Uint8Array, leafCount: bigint): boolean {
  return leafCount >= 1n && verifyMmrProof(proof, leafCount - 1n, mmrRoot, leafCount);
}

/**
 * Port of `storage_primitives::verify_mmr_proof`: the proof is for leaf
 * `leafIndex` of an MMR with `leafCount` leaves, the leaf hashes up to the
 * peak at its expected position, and the peaks bag to `mmrRoot`. A proof for
 * another leaf of the same MMR fails.
 */
export function verifyMmrProof(proof: MmrProof, leafIndex: bigint, mmrRoot: Uint8Array, leafCount: bigint): boolean {
  if (leafIndex < 0n || leafIndex >= leafCount) return false;

  // The leaf's peak, its index within that peak's subtree, and the subtree height.
  let remaining = leafCount;
  let offset = 0n;
  let peakIndex = 0;
  let height = 0;
  for (;;) {
    height = remaining.toString(2).length - 1;
    const subtreeLeaves = 1n << BigInt(height);
    if (leafIndex < offset + subtreeLeaves) break;
    offset += subtreeLeaves;
    remaining -= subtreeLeaves;
    peakIndex++;
  }
  const localIndex = leafIndex - offset;

  const peakCount = leafCount.toString(2).split("").filter((b) => b === "1").length;
  if (proof.siblings.length !== height || proof.path.length !== height || proof.peaks.length !== peakCount) {
    return false;
  }

  const leafBytes = concatBytes(proof.leaf.dataRoot, u64le(proof.leaf.dataSize), u64le(proof.leaf.totalSize));
  let current = hashLeaf(leafBytes);
  for (let k = 0; k < height; k++) {
    const isRight = proof.path[k];
    if (isRight !== (((localIndex >> BigInt(k)) & 1n) === 1n)) return false;
    current = isRight ? hashChildren(proof.siblings[k], current) : hashChildren(current, proof.siblings[k]);
  }
  if (!bytesEq(proof.peaks[peakIndex], current)) return false;
  return bytesEq(bagPeaks(proof.peaks), mmrRoot);
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
