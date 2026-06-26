// SPDX-License-Identifier: GPL-3.0-only

/**
 * Shared chain test-helpers for the three Web3-Storage UIs and the PAPI E2E
 * suite. This root export is Playwright-free so plain-node consumers
 * (examples/papi) can use it; Playwright fixtures live on the `./playwright`
 * subpath.
 */

export {
  getApi,
  getClient,
  disconnect as disconnectApi,
  submitExtrinsic,
  submitExtrinsicBestBlock,
  waitForBlock,
  getBlockNumber,
  getBestBlockNumber,
  type ParachainApi,
} from "./chain-api";

export {
  Alice,
  Bob,
  Charlie,
  Dave,
  Eve,
  Ferdie,
  devSigners,
  getDevSigner,
  type DevAccountName,
  type DevSigner,
} from "./signers";

export {
  registerProviderViaApi,
  deregisterProviderViaApi,
  cleanProviderRegistry,
  isProviderRegistered,
  type ProviderSettings,
  type RegistrationOptions,
  type RegistrationResult,
} from "./registration";

export {
  createBucketViaApi,
  deleteBucketViaApi,
  cleanupBuckets,
  createDriveViaApi,
  deleteDriveViaApi,
  cleanupDrives,
  type CreateBucketOptions,
  type BucketHandle,
  type CreateDriveOptions,
  type DriveHandle,
} from "./buckets";

export { ensureSoleAcceptingProvider } from "./orchestration";
