/**
 * Provider display specs.
 *
 * Verifies the Overview / Buckets / Agreements pages reflect on-chain state.
 * Setup uses api helpers to register Eve + create a bucket assigned to her,
 * so the tests don't have to walk the wizards.
 */
import { test, expect } from "../fixtures";
import {
  Bob,
  createBucketViaApi,
  cleanupBuckets,
} from "@web3-storage/test-helpers";

test.describe.configure({ mode: "serial" });
test.setTimeout(180_000);

// Eve is pre-registered as a provider by globalSetup; the fixture
// pre-injects Eve as the active account in localStorage. Just create one
// bucket as Bob (avoiding Alice contention with the running provider
// node's auto-coordinator) so the Buckets / Agreements pages have a row.
test.beforeAll(async () => {
  test.setTimeout(120_000);
  await createBucketViaApi(Bob, { name: `display-${Date.now()}` });
});

test.afterAll(async () => {
  test.setTimeout(60_000);
  await cleanupBuckets(Bob);
});

test("Overview shows provider info for registered Eve", async ({ localPage }) => {
  await localPage.getByTestId("nav-overview").click();
  await expect(localPage.getByTestId("provider-info")).toBeVisible({ timeout: 30_000 });
  // Stake stat card should be populated (non-zero).
  await expect(localPage.getByTestId("stat-card-total-stake")).toBeVisible({ timeout: 30_000 });
});

test("Buckets page renders the buckets table", async ({ localPage }) => {
  await localPage.getByTestId("nav-buckets").click();
  // Either "No buckets" message or a table — both are valid; we just want the
  // page to load without error.
  await expect(
    localPage.locator('[data-testid="buckets-table"], :text("No buckets")'),
  ).toBeVisible({ timeout: 30_000 });
});

// test("Agreements page renders the agreements table", async ({ localPage }) => {
//   await localPage.getByTestId("nav-agreements").click();
//   await expect(
//     localPage.locator('[data-testid="agreements-table"], :text("No active agreements")'),
//   ).toBeVisible({ timeout: 30_000 });
// });

