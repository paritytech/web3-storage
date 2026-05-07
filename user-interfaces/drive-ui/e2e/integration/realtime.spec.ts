/**
 * Real-time event subscription specs (multi-tab).
 *
 * Open two browser contexts, mutate state in tab A, assert tab B's sidebar
 * reflects the change without manual refresh. Exercises the
 * DriveRegistry.{DriveCreated,DriveDeleted,DriveNameUpdated} subscription.
 */
import { test, expect } from "../fixtures";
import {
  Alice,
  cleanupDrives,
  createDriveViaApi,
  deleteDriveViaApi,
  registerProviderViaApi,
  submitExtrinsic,
  getApi,
  waitForConnection,
  waitForMinBlock,
} from "@web3-storage/test-helpers";

test.describe.configure({ mode: "serial" });
test.setTimeout(180_000);

test.beforeAll(async () => {
  test.setTimeout(120_000);
  await registerProviderViaApi(Alice);
});

test.afterEach(async () => {
  await cleanupDrives(Alice);
});

async function openTabB(browser: import("@playwright/test").Browser) {
  // New context with the same localStorage seed as the fixture.
  const ctx = await browser.newContext();
  await ctx.addInitScript(() => {
    localStorage.setItem("web3-storage-selected-network", "local");
    localStorage.setItem("drive-ui-account-name", "Alice");
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
    const drive = await createDriveViaApi(Alice, {
      name: `rt-create-${Date.now()}`,
      maxCapacity: 10_000_000n,
      storagePeriod: 10_000,
      payment: 120_000_000_000_000_000n,
      minProviders: 1,
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
    const drive = await createDriveViaApi(Alice, {
      name: `rt-delete-${Date.now()}`,
      maxCapacity: 10_000_000n,
      storagePeriod: 10_000,
      payment: 120_000_000_000_000_000n,
      minProviders: 1,
    });
    await expect(tabB.getByTestId(`drive-list-item-${drive.driveId}`)).toBeVisible({
      timeout: 90_000,
    });

    await deleteDriveViaApi(Alice, drive.driveId);

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

// `update_drive_name` extrinsic + DriveNameUpdated event were removed in the
// pallet simplification — see `user-interfaces/PAPI_OVERHAUL.md`. The other
// two cross-tab events (DriveCreated / DriveDeleted) are still emitted, so
// only this one is fixmed.
test.fixme("DriveNameUpdated cross-tab", async ({ browser }) => {
  const { tabB, ctx: ctxB } = await openTabB(browser);
  try {
    const initial = `rt-rename-${Date.now()}`;
    const drive = await createDriveViaApi(Alice, {
      name: initial,
      maxCapacity: 10_000_000n,
      storagePeriod: 10_000,
      payment: 120_000_000_000_000_000n,
      minProviders: 1,
    });
    await expect(tabB.getByTestId(`drive-list-item-${drive.driveId}`)).toContainText(
      initial,
      { timeout: 60_000 },
    );

    const renamed = `${initial}-renamed`;
    await submitExtrinsic(
      getApi().tx.DriveRegistry.update_drive_name({
        drive_id: drive.driveId,
        name: new TextEncoder().encode(renamed),
      }),
      Alice.signer,
    );

    await expect(tabB.getByTestId(`drive-list-item-${drive.driveId}`)).toContainText(
      renamed,
      { timeout: 60_000 },
    );
  } finally {
    await ctxB.close();
  }
});
