// SPDX-License-Identifier: Apache-2.0

// TypeScript port of the Merkle functions the provider and the primitives
// use, so a client can compute and check CIDs itself without trusting the
// provider.
//
// Mirrors, byte for byte:
//   - `crates/providers/storage/src/index/fs.rs` → `metadata_merkle_root`
//   - `crates/primitives/storage/src/lib.rs`     → `blake2_256`, `hash_children`, `DEFAULT_CHUNK_SIZE`,
//                                                   `padded_merkle_tree`, `verify_mmr_proof`
//
// Pure functions, no I/O, browser-safe.

import { bytesEq } from "./bytes.js";
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
 * Identical to `storage_primitives::padded_merkle_tree(..).0`.
 */
export function paddedMerkleRoot(leaves: Uint8Array[]): Uint8Array {
  return paddedMerkleTree(leaves).root;
}

/** Internal node of a padded Merkle tree: `hash = hashChildren(left, right)`. */
export interface MerkleNode {
  hash: Uint8Array;
  left: Uint8Array;
  right: Uint8Array;
}

/**
 * Root and internal nodes of the padded Merkle tree over `leaves`, mirroring
 * `storage_primitives::padded_merkle_tree`. `nodes` is bottom-up, level by
 * level, left to right; a provider stores each one with data `left ++ right`
 * and children `[left, right]`. No leaves → zero root; one leaf → that leaf,
 * no nodes. Equal subtrees appear once per position, so a hash can repeat.
 */
export function paddedMerkleTree(leaves: Uint8Array[]): { root: Uint8Array; nodes: MerkleNode[] } {
  if (leaves.length === 0) return { root: new Uint8Array(32), nodes: [] };
  // Copy so the returned root never aliases the caller's input leaf buffer.
  if (leaves.length === 1) return { root: leaves[0].slice(), nodes: [] };

  let level = leaves.slice();
  const paddedLen = nextPowerOfTwo(level.length);
  while (level.length < paddedLen) level.push(new Uint8Array(32));

  const nodes: MerkleNode[] = [];
  while (level.length > 1) {
    const next: Uint8Array[] = [];
    for (let i = 0; i < level.length; i += 2) {
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

/** Number of chunks in a blob of `size` bytes (an empty blob has one). */
export function chunkCount(size: bigint | number): number {
  const n = Number(size);
  return n === 0 ? 1 : Math.ceil(n / DEFAULT_CHUNK_SIZE);
}

/**
 * A blob's `data_root`, which is also its CID: chunk the bytes at
 * `DEFAULT_CHUNK_SIZE`, blake2-256 each chunk, then `paddedMerkleRoot` over
 * the chunk hashes. An empty blob hashes a single empty chunk; a single chunk
 * yields its own hash.
 */
export function computeDataRoot(bytes: Uint8Array): Uint8Array {
  return paddedMerkleRoot(splitChunks(bytes).map((c) => computeCid(c)));
}

/**
 * Check a chunk proof from `GET /read` (the siblings of
 * `storage_primitives::padded_merkle_proof`) against `dataRoot`. The path
 * bits come from `index`. `chunks` is the blob's chunk count; it fixes the
 * expected sibling count, so a proof for a different tree shape fails.
 */
export function verifyChunkProof(
  chunkHash: Uint8Array,
  index: number,
  siblings: Uint8Array[],
  dataRoot: Uint8Array,
  chunks: number,
): boolean {
  if (index < 0 || index >= chunks) return false;
  if (siblings.length !== Math.log2(nextPowerOfTwo(chunks))) return false;
  let current = chunkHash;
  let idx = index;
  for (const sibling of siblings) {
    current = idx % 2 === 1 ? hashChildren(sibling, current) : hashChildren(current, sibling);
    idx = Math.floor(idx / 2);
  }
  return bytesEq(current, dataRoot);
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
 * {@link verifyMmrProof}, and also check that the proof is for the last leaf
 * of an MMR with `leafCount` leaves: the leaf is the right-most leaf of the
 * last peak, and the peak count matches `leafCount`. Without this check a
 * proof for an older leaf also passes.
 */
export function verifyLastMmrLeaf(proof: MmrProof, mmrRoot: Uint8Array, leafCount: bigint): boolean {
  if (leafCount < 1n || !verifyMmrProof(proof, mmrRoot)) return false;
  const peakCount = leafCount.toString(2).split("").filter((b) => b === "1").length;
  let lowestPeakHeight = 0;
  while (((leafCount >> BigInt(lowestPeakHeight)) & 1n) === 0n) lowestPeakHeight++;
  if (proof.peaks.length !== peakCount) return false;
  if (proof.siblings.length !== lowestPeakHeight || proof.path.length !== lowestPeakHeight) return false;
  if (!proof.path.every((right) => right)) return false;
  const leafBytes = concatBytes(proof.leaf.dataRoot, u64le(proof.leaf.dataSize), u64le(proof.leaf.totalSize));
  let current = computeCid(leafBytes);
  for (const sibling of proof.siblings) current = hashChildren(sibling, current);
  return bytesEq(current, proof.peaks[proof.peaks.length - 1]);
}

/**
 * Port of `storage_primitives::verify_mmr_proof`: the leaf hashes up to one
 * of the peaks, and the peaks bag to `mmrRoot`. Like the Rust function, it
 * does not check the leaf's index.
 */
export function verifyMmrProof(proof: MmrProof, mmrRoot: Uint8Array): boolean {
  const leafBytes = concatBytes(proof.leaf.dataRoot, u64le(proof.leaf.dataSize), u64le(proof.leaf.totalSize));
  let current = computeCid(leafBytes);
  proof.siblings.forEach((sibling, i) => {
    current = proof.path[i] ? hashChildren(sibling, current) : hashChildren(current, sibling);
  });
  if (!proof.peaks.some((p) => bytesEq(p, current))) return false;
  let bagged: Uint8Array | null = null;
  for (let i = proof.peaks.length - 1; i >= 0; i--) {
    bagged = bagged === null ? proof.peaks[i] : hashChildren(proof.peaks[i], bagged);
  }
  return bytesEq(bagged ?? new Uint8Array(32), mmrRoot);
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
