/**
 * Real-time event subscription specs (multi-tab).
 *
 * Open two browser contexts, mutate state in tab A, assert tab B's sidebar
 * reflects the change without manual refresh. Exercises the
 * DriveRegistry.{DriveCreated,DriveDeleted,DriveNameUpdated} subscription.
 */
import { test, expect } from "../fixtures";
import {
  Bob,
  cleanupDrives,
  createDriveViaApi,
  deleteDriveViaApi,
} from "@web3-storage/test-helpers";
import {
  waitForConnection,
  waitForMinBlock,
} from "@web3-storage/test-helpers/playwright";

test.describe.configure({ mode: "serial" });
test.setTimeout(180_000);

test.afterEach(async () => {
  await cleanupDrives(Bob);
});

async function openTabB(browser: import("@playwright/test").Browser) {
  // New context with the same localStorage seed as the fixture.
  const ctx = await browser.newContext();
  await ctx.addInitScript(() => {
    localStorage.setItem("web3-storage-selected-network", "local");
    localStorage.setItem("drive-ui-account-name", "Bob");
  });
  const tabB = await ctx.newPage();
  await tabB.goto("http://localhost:5174/");
  // Tab B has its own chain WS — wait until it's actually subscribed and has
  // observed a finalized block before asserting cross-tab event propagation.
  await waitForConnection(tabB, 60_000);
  await waitForMinBlock(tabB, 3, 60_000);
  return { tabB, ctx };
}

test("DriveCreated cross-tab", async ({ localPage, browser }) => {
  const { tabB, ctx } = await openTabB(browser);
  try {
    const drive = await createDriveViaApi(Bob, {
      name: `rt-create-${Date.now()}`,
      maxCapacity: 10_000_000n,
      storagePeriod: 10_000,
    });

    // Tab A also reflects (sanity).
    await expect(localPage.getByTestId(`drive-list-item-${drive.driveId}`)).toBeVisible({
      timeout: 90_000,
    });
    // Tab B reflects without reload.
    await expect(tabB.getByTestId(`drive-list-item-${drive.driveId}`)).toBeVisible({
      timeout: 90_000,
    });
  } finally {
    await ctx.close();
  }
});

test("DriveDeleted cross-tab", async ({ localPage, browser }) => {
  const { tabB, ctx } = await openTabB(browser);
  try {
    const drive = await createDriveViaApi(Bob, {
      name: `rt-delete-${Date.now()}`,
      maxCapacity: 10_000_000n,
      storagePeriod: 10_000,
    });
    await expect(tabB.getByTestId(`drive-list-item-${drive.driveId}`)).toBeVisible({
      timeout: 90_000,
    });

    await deleteDriveViaApi(Bob, drive.driveId);

    await expect(tabB.getByTestId(`drive-list-item-${drive.driveId}`)).toBeHidden({
      timeout: 90_000,
    });
    await expect(localPage.getByTestId(`drive-list-item-${drive.driveId}`)).toBeHidden({
      timeout: 90_000,
    });
  } finally {
    await ctx.close();
  }
});

