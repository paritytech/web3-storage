// SPDX-License-Identifier: Apache-2.0

/**
 * @web3-storage/layer1 — the two interchangeable storage interfaces (#123):
 * file-system drives and S3-style buckets, both over layer 0.
 */
export * from "./fs/index.js";
export * from "./s3/index.js";
export {
  ProviderUrlResolver,
  negotiateProviderTerms,
  type NegotiateProviderResult,
} from "./provider-url.js";
