/**
 * @web3-storage/layer1 — the two interchangeable storage interfaces (#123):
 * file-system drives and S3-style buckets, both over layer 0.
 */
export * from "./fs/index.js";
export * from "./s3/index.js";
export { ProviderUrlResolver } from "./provider-url.js";
