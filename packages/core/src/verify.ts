// SPDX-License-Identifier: Apache-2.0

/**
 * Content-addressing verification. A blob's CID is its `data_root`, the root
 * of the Merkle tree over its chunk hashes. A blob of at most one chunk has
 * the chunk's leaf hash (`blake2b-256(0x00 ++ data)`) as its root (see the
 * single-leaf case of `merkle_root` in crates/primitives/storage/src/merkle.rs),
 * so it can be verified here directly. Multi-chunk blobs are checked chunk by
 * chunk against their Merkle tree (`readBlob` in @web3-storage/storage).
 */

import { asHex, base64ToBytes, hexToBytes, toHex } from "./bytes.js";
import { hashChildren, hashLeaf } from "./merkle.js";

export class CidMismatchError extends Error {
  readonly expected: string;
  readonly actual: string;
  constructor(expected: string, actual: string) {
    super(
      `Content does not match its CID (expected ${expected}, got ${actual}) — ` +
        `the provider served corrupted or substituted bytes`,
    );
    this.name = "CidMismatchError";
    this.expected = expected;
    this.actual = actual;
  }
}

/** CID of `data` of at most one chunk: its Merkle leaf hash. Use `computeDataRoot` for larger data. */
export function computeCid(data: Uint8Array): Uint8Array {
  return hashLeaf(data);
}

/** Throw {@link CidMismatchError} unless `data` hashes to `expectedCid`. */
export function verifyCid(data: Uint8Array, expectedCid: string | Uint8Array): void {
  const expected = asHex(expectedCid).toLowerCase();
  const actual = toHex(computeCid(data)).toLowerCase();
  if (expected !== actual) {
    throw new CidMismatchError(expected, actual);
  }
}

/** A chunk-tree node: a chunk's bytes, or an internal node's two child hashes. */
export type TreeNode = { data: Uint8Array } | { children: [Uint8Array, Uint8Array] };

const HASH_HEX = /^(0x)?[0-9a-fA-F]{64}$/;

/**
 * Parse a provider `/node` body: base64 `data` for a chunk, or two hex
 * `children` for an internal node. Returns null when the body has both or
 * neither, a wrong child count, or a child that is not a 32-byte hex hash.
 * The length check matters: `hashChildren` hashes `left ++ right`, so children
 * with a shifted boundary would still match the parent hash.
 */
export function parseNode(body: {
  data?: string | null;
  children?: string[] | null;
}): TreeNode | null {
  const { data, children } = body;
  if (data != null && children == null) return { data: base64ToBytes(data) };
  if (data == null && children?.length === 2 && children.every((c) => HASH_HEX.test(c))) {
    const [left, right] = children.map(hexToBytes);
    return { children: [left, right] };
  }
  return null;
}

/** Hash of a node: a leaf hash over a chunk's bytes, a node hash over two children. */
export function nodeHash(node: TreeNode): Uint8Array {
  return "data" in node ? hashLeaf(node.data) : hashChildren(...node.children);
}
