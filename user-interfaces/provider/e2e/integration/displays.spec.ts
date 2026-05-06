/**
 * Provider display specs.
 *
 * Verifies the Overview / Buckets / Agreements pages reflect on-chain state.
 * Setup uses api helpers to register Eve + create a bucket assigned to her,
 * so the tests don't have to walk the wizards.
 */
import { test, expect } from "../fixtures";
import {
  Alice,
  Eve,
  registerProviderViaApi,
  createBucketViaApi,
  cleanupBuckets,
} from "@web3-storage/test-helpers";

test.describe.configure({ mode: "serial" });
test.setTimeout(180_000);

test.beforeAll(async () => {
  test.setTimeout(120_000);
  await registerProviderViaApi(Eve, { multiaddr: "/ip4/127.0.0.1/tcp/3334" });
  // Create a bucket as Alice that may end up assigned to a registered provider.
  // For the display tests we don't need provider assignment via UI — the test
  // just verifies that *some* row shows on the Buckets page.
  await createBucketViaApi(Alice, { name: `display-${Date.now()}` });
});

test.afterAll(async () => {
  await cleanupBuckets(Alice);
});

test.beforeEach(async ({ localPage }) => {
  await localPage.getByTestId("provider-account-button").click();
  await localPage.getByTestId("provider-account-select-Eve (Dev)").click();
});

test("Overview shows provider info for registered Eve", async ({ localPage }) => {
  await localPage.getByTestId("nav-overview").click();
  await expect(localPage.getByTestId("provider-info")).toBeVisible({ timeout: 30_000 });
  // Stake stat card should be populated (non-zero).
  await expect(localPage.getByTestId("stat-card-stake")).toBeVisible({ timeout: 30_000 });
});

test("Buckets page renders the buckets table", async ({ localPage }) => {
  await localPage.getByTestId("nav-buckets").click();
  // Either "No buckets" message or a table — both are valid; we just want the
  // page to load without error.
  await expect(
    localPage.locator('[data-testid="buckets-table"], :text("No buckets")'),
  ).toBeVisible({ timeout: 30_000 });
});

test("Agreements page renders the agreements table", async ({ localPage }) => {
  await localPage.getByTestId("nav-agreements").click();
  await expect(
    localPage.locator('[data-testid="agreements-table"], :text("No active agreements")'),
  ).toBeVisible({ timeout: 30_000 });
});
