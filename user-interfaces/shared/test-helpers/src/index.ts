/**
 * Shared Playwright + chain helpers for the three Web3-Storage UIs
 * (drive-ui, console-ui, provider).
 */

export { expect } from "@playwright/test";

export {
  makeLocalPageFixture,
  waitForConnection,
  waitForMinBlock,
  probeProviderHealth,
  teardownChain,
  type LocalPageFixtureOptions,
} from "./fixtures";

export {
  getApi,
  getClient,
  disconnect as disconnectApi,
  submitExtrinsic,
  waitForBlock,
  getBlockNumber,
  type ParachainApi,
  type SubmitResult,
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
