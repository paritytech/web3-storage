// SPDX-License-Identifier: GPL-3.0-only

/**
 * Playwright-coupled half of the test-helpers: fixtures and page-level
 * assertions. Separate subpath (`@web3-storage/test-helpers/playwright`) so
 * the Playwright-free root export can be consumed by plain-node suites.
 */

export { expect } from "@playwright/test";

export {
  makeLocalPageFixture,
  waitForConnection,
  waitForMinBlock,
  expectBestBlockToAdvance,
  probeProviderHealth,
  teardownChain,
  type LocalPageFixtureOptions,
} from "./fixtures";
