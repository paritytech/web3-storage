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
  deleteDriveViaApi,
  waitForConnection,
  waitForMinBlock,
} from "@web3-storage/test-helpers";
import { createDriveViaUi } from "../helpers/createDriveViaUi";

test.describe.configure({ mode: "serial" });
test.setTimeout(180_000);

test.afterEach(async () => {
  test.setTimeout(180_000);
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
    const driveId = await createDriveViaUi(localPage, `rt-create-${Date.now()}`);

    await expect(localPage.getByTestId(`drive-list-item-${driveId}`)).toBeVisible({
      timeout: 90_000,
    });
    await expect(tabB.getByTestId(`drive-list-item-${driveId}`)).toBeVisible({
      timeout: 90_000,
    });
  } finally {
    await ctx.close();
  }
});

test("DriveDeleted cross-tab", async ({ localPage, browser }) => {
  const { tabB, ctx } = await openTabB(browser);
  try {
    const driveId = await createDriveViaUi(localPage, `rt-delete-${Date.now()}`);
    await expect(tabB.getByTestId(`drive-list-item-${driveId}`)).toBeVisible({
      timeout: 90_000,
    });

    await deleteDriveViaApi(Bob, driveId);

    await expect(tabB.getByTestId(`drive-list-item-${driveId}`)).toBeHidden({
      timeout: 90_000,
    });
    await expect(localPage.getByTestId(`drive-list-item-${driveId}`)).toBeHidden({
      timeout: 90_000,
    });
  } finally {
    await ctx.close();
  }
});

