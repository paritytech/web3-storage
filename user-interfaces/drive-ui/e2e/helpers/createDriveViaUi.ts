// SPDX-License-Identifier: GPL-3.0-only

import { expect, type Browser, type Page } from "@playwright/test";
import { isSameAddress } from "@web3-storage/sdk";
import { Bob, getApi, getBestBlockNumber } from "@web3-storage/test-helpers";

/**
 * Drive the drive-ui through the real user create-drive flow:
 *
 *   1. Click "New Drive" → form opens with the provider picker embedded.
 *   2. Set the price (capacity / duration defaults stay).
 *   3. Click the first available provider's Select button — this IS the
 *      submit. The UI runs `POST /negotiate` then submits
 *      `create_bucket_with_primary`.
 *   4. Wait for a `StorageProvider.BucketCreated` event with Bob as admin.
 *
 * Returns the new bucket id. For tests that only need a drive as setup
 * state and don't care about the UI flow, `createDriveViaApi` is faster.
 */
export async function createDriveViaUi(page: Page): Promise<bigint> {
  await page.getByTestId("new-drive-button").click();
  await expect(page.getByTestId("new-drive-dialog")).toBeVisible();
  await page.getByTestId("new-drive-price").fill("100");
  await page.getByTestId("find-matching-providers").click();
  // Capacity / duration defaults are set in the component.
  await expect(page.getByTestId("provider-picker")).toBeVisible({ timeout: 30_000 });

  const currentBlock = await getBestBlockNumber();
  await page.getByTestId("provider-picker-select").first().click();
  return waitForBucketCreatedBy(Bob.address, currentBlock);
}

/**
 * `beforeAll` variant — opens a fresh browser context (with the same
 * localStorage seed as the `localPage` fixture: local network + Bob
 * signer), navigates to the app, drives the UI create flow, and closes
 * the context. Returns the new bucket id.
 *
 * Use from `test.beforeAll` when you need a drive created via the UI
 * before any individual test runs.
 */
export async function createDriveInFreshContext(browser: Browser): Promise<bigint> {
  const context = await browser.newContext();
  const page = await context.newPage();
  await page.addInitScript(() => {
    localStorage.setItem("web3-storage-selected-network", "local");
    localStorage.setItem("drive-ui-account-name", "Bob");
  });
  try {
    await page.goto("/");
    await expect(page.getByTestId("block-number")).toBeVisible({ timeout: 30_000 });
    return await createDriveViaUi(page);
  } finally {
    await context.close();
  }
}

/**
 * Resolve with the id of the first bucket created by `admin` in a block
 * after `afterBlock`.
 */
export async function waitForBucketCreatedBy(admin: string, afterBlock: number): Promise<bigint> {
  const api = getApi();
  return new Promise<bigint>((resolve, reject) => {
    const sub = api.event.StorageProvider.BucketCreated.watch().subscribe({
      next: ({ block, events }) => {
        if (block.number <= afterBlock) return;
        const created = events.find(({ payload }) => isSameAddress(payload.admin, admin));
        if (!created) return;
        sub.unsubscribe();
        resolve(created.payload.bucket_id);
      },
      error: reject,
    });
  });
}
