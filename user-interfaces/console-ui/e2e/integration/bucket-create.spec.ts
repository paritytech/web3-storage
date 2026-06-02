/**
 * Bucket creation spec (slow ~30s).
 *
 * Walks the Storage page UI through the two-step create flow:
 *   1. Fill the form and click "Choose Provider & Create" — opens picker
 *   2. Pick the first available provider — UI negotiates over HTTP and
 *      submits `create_s3_bucket` atomically
 *
 * Then verifies the bucket appears in the selector AND on-chain at
 * `S3Registry.S3Buckets`.
 */
import { test, expect } from "../fixtures";
import {
  Alice,
  cleanupBuckets,
  getApi,
} from "@web3-storage/test-helpers";

test.describe.configure({ mode: "serial" });
test.setTimeout(180_000);

// Alice is registered as the provider in playwright globalSetup; no
// per-spec beforeAll registration needed.

test.afterAll(async () => {
  test.setTimeout(90_000);
  await cleanupBuckets(Alice);
});

test("create bucket via UI → on-chain S3Buckets matches", async ({ localPage }) => {
  await localPage.getByTestId("nav-storage").click();
  // The form auto-opens when there are no buckets; otherwise click the New Bucket button.
  if (!(await localPage.getByTestId("s3-create-bucket-form").isVisible().catch(() => false))) {
    await localPage.getByTestId("s3-new-bucket").click();
  }
  await expect(localPage.getByTestId("s3-create-bucket-form")).toBeVisible();

  const name = `bucket-create-${Date.now()}`;
  await localPage.getByTestId("s3-bucket-name-input").fill(name);
  // Defaults for capacity / duration / price-per-byte are set in the component.
  await localPage.getByTestId("s3-create-submit").click();

  // Step 1 of the new flow: the picker opens with the list of available
  // providers. Globalsetup registered Alice as the only provider, so we
  // can confidently click the first Select button.
  await expect(localPage.getByTestId("provider-picker")).toBeVisible({
    timeout: 30_000,
  });
  await localPage.getByTestId("provider-picker-select").first().click();

  // Step 2 runs automatically (HTTP negotiate → on-chain submit). Wait for
  // the new bucket to appear in the selector — the UI auto-selects it on
  // success.
  await expect(localPage.getByTestId("s3-bucket-selector")).toContainText(name, {
    timeout: 90_000,
  });

  // Verify on chain. The UI submits at best-block, but test-helpers' getApi
  // is a separate ws connection — finalization can lag, so poll instead of
  // doing an immediate getValue.
  const api = getApi();
  await expect.poll(
    async () => {
      const ids = await api.query.S3Registry.UserBuckets.getValue(Alice.address);
      return ids?.length ?? 0;
    },
    { timeout: 60_000, intervals: [1000, 2000, 3000] },
  ).toBeGreaterThan(0);

  const userBuckets = await api.query.S3Registry.UserBuckets.getValue(Alice.address);
  const latestId = userBuckets![userBuckets!.length - 1];
  const bucket = await api.query.S3Registry.S3Buckets.getValue(latestId);
  expect(bucket).toBeTruthy();
  const bucketName = new TextDecoder().decode(bucket!.name);
  expect(bucketName).toBe(name);
});
