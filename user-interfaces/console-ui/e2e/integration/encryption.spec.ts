/**
 * Encryption spec (UI-only, fast).
 *
 * Toggle reveals/hides the key field; key validation gates the Enable button;
 * Generate populates a valid 64-hex-char key.
 */
import { test, expect } from "../fixtures";
import { Alice, cleanupBuckets } from "@web3-storage/test-helpers";
import { createBucketInFreshContext } from "../helpers/createBucketViaUi";

test.describe.configure({ mode: "serial" });
test.setTimeout(180_000);

let bucketName: string;

test.beforeAll(async ({ browser }) => {
  test.setTimeout(180_000);
  bucketName = `enc-${Date.now()}`;
  await createBucketInFreshContext(browser, bucketName);
});

test.afterAll(async () => {
  test.setTimeout(180_000);
  await cleanupBuckets(Alice);
});

test.beforeEach(async ({ localPage }) => {
  await localPage.getByTestId("nav-storage").click();
  await localPage.getByTestId("s3-bucket-selector").selectOption({ label: bucketName });
});

test("toggle reveals/hides the encryption key form", async ({ localPage }) => {
  await localPage.getByTestId("s3-encryption-toggle").click();
  await expect(localPage.getByTestId("s3-encryption-form")).toBeVisible();

  await localPage.getByTestId("s3-encryption-cancel").click();
  await expect(localPage.getByTestId("s3-encryption-form")).toBeHidden();
});

test("key validation: 63 chars rejects, 64 hex chars accepts, Generate populates valid key", async ({ localPage }) => {
  await localPage.getByTestId("s3-encryption-toggle").click();
  const input = localPage.getByTestId("s3-encryption-key-input");
  const enable = localPage.getByTestId("s3-encryption-enable");

  // 63 hex chars is rejected (truncated input would still be 63 → enable disabled).
  await input.fill("a".repeat(63));
  await expect(enable).toBeDisabled();

  // 64 hex chars accepted.
  await input.fill("a".repeat(64));
  await expect(enable).toBeEnabled();

  // Generate populates a fresh 64-hex-char key.
  await input.fill("");
  await localPage.getByTestId("s3-encryption-generate").click();
  await expect(input).toHaveValue(/^[0-9a-fA-F]{64}$/);
  await expect(enable).toBeEnabled();
});
