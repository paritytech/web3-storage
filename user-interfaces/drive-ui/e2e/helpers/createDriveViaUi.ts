import { expect, type Browser, type Page } from "@playwright/test";
import { Bob, getApi } from "@web3-storage/test-helpers";

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
  // Capacity / duration / price-per-byte defaults are set in the component.
  await expect(page.getByTestId("provider-picker")).toBeVisible({ timeout: 30_000 });
  await page.getByTestId("provider-picker-select").first().click();

  return waitForLatestDriveId();
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
  await context.addInitScript(() => {
    localStorage.setItem("web3-storage-selected-network", "local");
    localStorage.setItem("drive-ui-account-name", "Bob");
  });
  const page = await context.newPage();
  try {
    await page.goto("/");
    await expect(page.getByTestId("block-number")).toBeVisible({ timeout: 30_000 });
    return await createDriveViaUi(page, name);
  } finally {
    await context.close();
  }
}

/**
 * Poll `DriveRegistry.UserDrives[Bob]` until it has at least one entry,
 * then return the latest id. Used after the UI flow signals completion
 * to bridge the UI's optimistic state and the chain's finalized state.
 */
async function waitForLatestDriveId(): Promise<bigint> {
  const api = getApi();
  let length = 0;
  await expect
    .poll(
      async () => {
        const ids = await api.query.DriveRegistry.UserDrives.getValue(Bob.address);
        length = ids.length;
        return length;
      },
      { timeout: 120_000, intervals: [1000, 2000, 3000] },
    )
    .toBeGreaterThan(0);
  const ids = await api.query.DriveRegistry.UserDrives.getValue(Bob.address);
  return ids[ids.length - 1];
}
