// SPDX-License-Identifier: Apache-2.0

/**
 * @web3-storage/sdk: umbrella over @web3-storage/core, layer0 and
 * file-system, plus the Web3Storage class. pallet_revive helpers stay on the
 * ./revive subpath, so consumers that never touch contracts do not load viem.
 */

export * from "@web3-storage/layer0";
export * from "@web3-storage/file-system";

// core's chain-free primitives that layer0 doesn't already re-export.
export {
  h160ToSubstrate,
  type MappedAccount,
  HttpError,
  httpFetch,
  negotiateTerms,
  bytesToBase64,
  base64ToBytes,
  signProviderRequest,
  computeCid,
  hashLeaf,
  verifyCid,
  CidMismatchError,
  DEFAULT_CHUNK_SIZE,
  computeDataRoot,
  metadataMerkleRoot,
  merkleRoot,
  leafChunkCount,
  leafChunkSize,
  LEAF_VERSION,
  hashChildren,
  merkleTree,
  bagPeaks,
  splitChunks,
  verifyChunkProof,
  verifyMmrProof,
  createLimiter,
  u64le,
  type MerkleNode,
  type MmrLeaf,
  type MmrProof,
  type HttpFetchOpts,
  type MerkleEntry,
  type BucketTargetWire,
  type NegotiateRequest,
  type SignedTerms,
  type ProviderRequestSigner,
} from "@web3-storage/core";

export { Web3Storage, type CreateBucketOptions, type Web3StorageOptions } from "./web3-storage.js";
