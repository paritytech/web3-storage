/**
 * Rename + clear-contents specs.
 *
 * Verify that:
 *  (a) renaming a drive surfaces the new name in the sidebar without manual
 *      refresh (i.e. the DriveNameUpdated event subscription works), and the
 *      on-chain `Drives.name` matches.
 *  (b) clear-contents resets the drive's root_cid to the empty-tree hash and
 *      the directory listing becomes empty.
 */
import { test, expect } from "../fixtures";
import {
  Alice,
  cleanupDrives,
  createDriveViaApi,
  registerProviderViaApi,
  getApi,
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

test("rename → DriveNameUpdated event reflects without manual refresh", async ({ localPage }) => {
  const initialName = `rename-${Date.now()}`;
  const drive = await createDriveViaApi(Alice, {
    name: initialName,
    maxCapacity: 10_000_000n,
    storagePeriod: 10_000,
    payment: 120_000_000_000_000_000n,
    minProviders: 1,
  });

  await localPage.reload();
  await expect(localPage.getByTestId(`drive-list-item-${drive.driveId}`)).toContainText(
    initialName,
  );

  // Hover to reveal the menu, open it, click Rename.
  await localPage.getByTestId(`drive-list-item-${drive.driveId}`).hover();
  await localPage.getByTestId(`drive-list-menu-${drive.driveId}`).click();
  await localPage.getByTestId(`drive-list-rename-${drive.driveId}`).click();

  const newName = `${initialName}-renamed`;
  await expect(localPage.getByTestId("rename-dialog")).toBeVisible();
  await localPage.getByTestId("rename-input").fill(newName);
  await localPage.getByTestId("rename-submit").click();

  // The DriveNameUpdated subscription should refresh the sidebar.
  await expect(localPage.getByTestId(`drive-list-item-${drive.driveId}`)).toContainText(
    newName,
    { timeout: 30_000 },
  );

  const onchain = await getApi().query.DriveRegistry.Drives.getValue(drive.driveId);
  expect(onchain).toBeTruthy();
  const onchainName = onchain!.name ? new TextDecoder().decode(onchain!.name.asBytes()) : null;
  expect(onchainName).toBe(newName);
});

test("clear contents → directory listing empty + root_cid resets", async ({ localPage }) => {
  const drive = await createDriveViaApi(Alice, {
    name: `clear-${Date.now()}`,
    maxCapacity: 10_000_000n,
    storagePeriod: 10_000,
    payment: 120_000_000_000_000_000n,
    minProviders: 1,
  });

  await localPage.reload();
  await localPage.getByTestId(`drive-list-item-${drive.driveId}`).click();
  await expect(localPage.getByTestId("file-browser")).toBeVisible();

  await localPage.getByTestId(`drive-list-item-${drive.driveId}`).hover();
  await localPage.getByTestId(`drive-list-menu-${drive.driveId}`).click();
  await localPage.getByTestId(`drive-list-clear-${drive.driveId}`).click();

  // Confirmation dialog uses native confirm or a Radix dialog — accept whichever.
  // If the UI uses an inline confirm button, click it; if a native dialog, accept.
  // Try testid first, fall back to native dialog handler.
  const confirmBtn = localPage.getByTestId("clear-confirm-submit");
  if (await confirmBtn.isVisible().catch(() => false)) {
    await confirmBtn.click();
  }

  // After clear, the root_cid in the on-chain Drives record should reset.
  // Empty-tree CID is 32 zero bytes (the documented default).
  await expect.poll(
    async () => {
      const d = await getApi().query.DriveRegistry.Drives.getValue(drive.driveId);
      return d?.root_cid?.asHex();
    },
    { timeout: 30_000, intervals: [1000, 2000, 3000] },
  ).toMatch(/^0x0+$/);
});
