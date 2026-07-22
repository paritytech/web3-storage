// SPDX-License-Identifier: Apache-2.0

/**
 * @web3-storage/sdk — umbrella over the #123 package layout. Everything
 * consumers used before the split keeps resolving from here; the physical
 * core/layer0/layer1 boundaries are an implementation detail.
 *
 * pallet_revive helpers stay on the ./revive subpath (viem isolation);
 * layer-1 clients are also importable directly via ./fs and ./s3.
 */

export * from "@web3-storage/layer0";
export * from "@web3-storage/layer1";

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
  verifyCid,
  CidMismatchError,
  DEFAULT_CHUNK_SIZE,
  computeDataRoot,
  metadataMerkleRoot,
  paddedMerkleRoot,
  hashChildren,
  u64le,
  type HttpFetchOpts,
  type MerkleEntry,
  type NegotiateRequest,
  type SignedTerms,
  type ProviderRequestSigner,
} from "@web3-storage/core";

export { Web3Storage, type Web3StorageOptions } from "./web3-storage.js";
