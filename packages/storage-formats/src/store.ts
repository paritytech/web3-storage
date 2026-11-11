// SPDX-License-Identifier: Apache-2.0

/** The blob store of the wasm tree, over the provider's HTTP API. */

import { hexToBytes } from "@web3-storage/core";
import {
  commitDataRoots,
  getCommitment,
  getMmrProof,
  readBlob,
  uploadBlob,
  type ChainSigner,
  type ProviderHttpOpts,
} from "@web3-storage/layer0";

import type { BlobStore } from "../pkg/storage_formats.js";

/** A {@link BlobStore} of one bucket. */
export interface BucketBlobStore extends BlobStore {
  readonly bucketId: bigint;
}

/**
 * {@link BlobStore} over the provider's HTTP API. `signer` signs writes and
 * the root lookup of a private bucket (`GET /commitment`, `GET /mmr_proof`).
 *
 * A read of unknown length requests up to `length + 1` bytes, so that the tree
 * can reject a longer blob. The tree checks the MMR proof of the last leaf
 * against the provider's commitment.
 */
export function providerBlobStore(
  providerUrl: string,
  bucketId: bigint,
  signer: ChainSigner | null,
  http: ProviderHttpOpts = {},
): BucketBlobStore {
  const reads = { ...http, signer: signer ?? undefined };
  const requireSigner = () => {
    if (!signer) throw new Error("Signer not set");
    return signer;
  };
  return {
    bucketId,
    getBlob: (cid, length, exact) =>
      readBlob(providerUrl, bucketId, cid, exact ? { ...http, size: length } : { ...http, maxSize: Number(length) + 1 }),
    async putBlob(bytes) {
      const result = await uploadBlob(providerUrl, bucketId, bytes, requireSigner(), http);
      return hexToBytes(result.dataRoot);
    },
    async commit(cids) {
      await commitDataRoots(providerUrl, bucketId, cids, requireSigner(), http);
    },
    commitment: () => getCommitment(providerUrl, bucketId, reads),
    mmrProof: (commitment, leafIndex) => getMmrProof(providerUrl, bucketId, commitment, leafIndex, reads),
  };
}
