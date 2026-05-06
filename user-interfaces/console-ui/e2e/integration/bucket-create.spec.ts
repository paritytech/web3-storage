/**
 * Bucket creation spec (slow ~30s).
 *
 * Walks the Storage page UI to create a bucket, then verifies the bucket
 * appears in the selector AND on-chain at S3Registry.S3Buckets.
 */
import { test, expect } from "../fixtures";
import {
  Alice,
  cleanupBuckets,
  registerProviderViaApi,
  getApi,
} from "@web3-storage/test-helpers";

test.describe.configure({ mode: "serial" });
test.setTimeout(180_000);

test.beforeAll(async () => {
  test.setTimeout(120_000);
  await registerProviderViaApi(Alice);
});

test.afterAll(async () => {
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
  // Use defaults for capacity / duration / max-payment (set in the component).
  await localPage.getByTestId("s3-create-submit").click();

  // Wait for the new bucket to appear in the selector. After creation the UI
  // auto-selects it.
  await expect(localPage.getByTestId("s3-bucket-selector")).toContainText(name, {
    timeout: 90_000,
  });

  // Verify on chain.
  const api = getApi();
  const userBuckets = await api.query.S3Registry.UserBuckets.getValue(Alice.address);
  expect(userBuckets).toBeTruthy();
  expect(userBuckets!.length).toBeGreaterThan(0);

  const latestId = userBuckets![userBuckets!.length - 1];
  const bucket = await api.query.S3Registry.S3Buckets.getValue(latestId);
  expect(bucket).toBeTruthy();
  const bucketName = new TextDecoder().decode(bucket!.name.asBytes());
  expect(bucketName).toBe(name);
});
