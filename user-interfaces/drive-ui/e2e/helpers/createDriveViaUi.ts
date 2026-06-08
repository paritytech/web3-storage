import { expect, type Browser, type Page } from "@playwright/test";
import { getApi, getBestBlockNumber } from "@web3-storage/test-helpers";

/**
 * Drive the drive-ui through the real user create-drive flow:
 *
 *   1. Click "New Drive" → form opens with the provider picker embedded.
 *   2. Fill the drive name (capacity / duration / price defaults stay).
 *   3. Click the first available provider's Select button — this IS the
 *      submit. The UI runs `POST /negotiate` then submits `create_drive`
 *      atomically.
 *   4. Wait for the new drive id to appear under `Bob` in
 *      `DriveRegistry.UserDrives`.
 *
 * Returns the new drive's id. For tests that only need a drive as setup
 * state and don't care about the UI flow, `createDriveViaApi` is faster.
 */
export async function createDriveViaUi(page: Page, name: string): Promise<bigint> {
  await page.getByTestId("new-drive-button").click();
  await expect(page.getByTestId("new-drive-dialog")).toBeVisible();
  await page.getByTestId("new-drive-name").fill(name);
  await page.getByTestId("new-drive-price").fill("100");
  await page.getByTestId("find-matching-providers").click();
  // Capacity / duration defaults are set in the component.
  await expect(page.getByTestId("provider-picker")).toBeVisible({ timeout: 30_000 });

  const currentBlock = await getBestBlockNumber();
  await page.getByTestId("provider-picker-select").first().click();
  return waitForLatestDriveId(currentBlock);
}

/**
 * `beforeAll` variant — opens a fresh browser context (with the same
 * localStorage seed as the `localPage` fixture: local network + Bob
 * signer), navigates to the app, drives the UI create flow, and closes
 * the context. Returns the new drive's id.
 *
 * Use from `test.beforeAll` when you need a drive created via the UI
 * before any individual test runs.
 */
export async function createDriveInFreshContext(
  browser: Browser,
  name: string,
): Promise<bigint> {
  const context = await browser.newContext();
  const page = await context.newPage();
  await page.addInitScript(() => {
    localStorage.setItem("web3-storage-selected-network", "local");
    localStorage.setItem("drive-ui-account-name", "Bob");
  });
  try {
    await page.goto("/");
    await expect(page.getByTestId("block-number")).toBeVisible({ timeout: 30_000 });
    return await createDriveViaUi(page, name);
  } finally {
    await context.close();
  }
}

export async function waitForLatestDriveId(currentBlock: number): Promise<bigint> {
  const api = getApi();
  return new Promise<bigint>((resolve, reject) => {
    const sub = api.event.DriveRegistry.DriveCreated.watch().subscribe({
      next: ({ block, events }) => {
        if (block.number <= currentBlock) return;
        if (events.length === 0) return;
        sub.unsubscribe();
        resolve(events[0].payload.drive_id);
      },
      error: reject,
    });
  });
}
