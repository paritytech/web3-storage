// SPDX-License-Identifier: Apache-2.0

/**
 * @web3-storage/layer1 — the two interchangeable storage interfaces (#123):
 * file-system drives and S3-style buckets, both plain layer 0 buckets.
 */
export * from "./fs/index.js";
export * from "./s3/index.js";
export { getBucketInfos, listMemberBuckets } from "./bucket-info.js";
export {
  ProviderUrlResolver,
  negotiateProviderTerms,
  type NegotiateProviderResult,
} from "./provider-url.js";
