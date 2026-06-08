import { expect, type Browser, type Page } from "@playwright/test";
import { getApi, getBestBlockNumber } from "@web3-storage/test-helpers";

/**
 * Drive the console-ui through the real user create-bucket flow:
 *
 *   1. Open the Storage tab and the create form (auto-opens when there are
 *      no buckets; otherwise click "New Bucket").
 *   2. Fill the bucket name (capacity / duration / price defaults stay).
 *   3. Click "Choose Provider & Create" — opens the picker dialog.
 *   4. Click the first available provider's Select button. The UI runs the
 *      HTTP `POST /negotiate` step and then submits `create_s3_bucket`
 *      atomically.
 *   5. Wait for the new bucket to appear in the selector.
 *
 */
export async function createBucketViaUi(page: Page, name: string): Promise<void> {
  await page.getByTestId("nav-storage").click();
  if (!(await page.getByTestId("s3-create-bucket-form").isVisible().catch(() => false))) {
    await page.getByTestId("s3-new-bucket").click();
  }
  await expect(page.getByTestId("s3-create-bucket-form")).toBeVisible();

  await page.getByTestId("s3-bucket-name-input").fill(name);
  await page.getByTestId("s3-bucket-price-input").fill("100");
  await page.getByTestId("s3-create-submit").click();

  await expect(page.getByTestId("provider-picker")).toBeVisible({ timeout: 30_000 });
  await page.getByTestId("provider-picker-select").first().click();

  const currentBlockNumber = await getBestBlockNumber();
  await waitForLatestBucketId(currentBlockNumber);

  await expect(page.getByTestId("s3-bucket-selector")).toContainText(name, {
    timeout: 90_000,
  });
}

/**
 * `beforeAll` variant — opens a fresh browser context (with the same
 * localStorage seed as the `localPage` fixture), navigates to the app,
 * drives the UI create flow, and closes the context. Use from `test.beforeAll`
 * when you need a bucket created via the UI before any individual test runs.
 */
export async function createBucketInFreshContext(
  browser: Browser,
  name: string,
): Promise<void> {
  const context = await browser.newContext();
  const page = await context.newPage();
  await page.addInitScript(() => {
    localStorage.setItem("web3-storage-selected-network", "local");
  });
  try {
    await page.goto("/");
    await expect(page.getByTestId("block-number")).toBeVisible({ timeout: 30_000 });
    await createBucketViaUi(page, name);
  } finally {
    await context.close();
  }
}

export async function waitForLatestBucketId(currentBlock: number): Promise<bigint> {
  const api = getApi();
  return new Promise<bigint>((resolve, reject) => {
    const sub = api.event.S3Registry.S3BucketCreated.watch().subscribe({
      next: ({ block, events }) => {
        if (block.number <= currentBlock) return;
        if (events.length === 0) return;
        sub.unsubscribe();
        resolve(events[0].payload.s3_bucket_id);
      },
      error: reject,
    });
  });
}
