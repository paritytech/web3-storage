/**
 * Rename + clear-contents specs — currently both `test.fixme`.
 *
 * The file-system pallet was simplified to 4 extrinsics
 * (`create_drive` / `delete_drive` / `share_drive` / `unshare_drive`); the
 * `update_drive_name` and `clear_drive` extrinsics drive-ui's UI calls
 * don't exist on the runtime anymore. Drive-ui still ships the
 * RenameDriveDialog and the Clear context-menu item, so the UI surface
 * is testable in principle, but the on-chain assertions can't be true
 * until either:
 *   (a) the runtime extrinsics come back, or
 *   (b) drive-ui drops these surfaces and we delete these specs.
 *
 * Tracked as part of the drive-ui catch-up flagged in
 * `user-interfaces/PAPI_OVERHAUL.md`.
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

test.fixme("rename → DriveNameUpdated event reflects without manual refresh", async ({ localPage }) => {
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

  // The DriveNameUpdated subscription should refresh the sidebar. The
  // event-driven refresh kicks in once the rename tx is finalized; on a
  // fresh chain that round-trip can take ~30-60s.
  await expect(localPage.getByTestId(`drive-list-item-${drive.driveId}`)).toContainText(
    newName,
    { timeout: 90_000 },
  );

  const onchain = await getApi().query.DriveRegistry.Drives.getValue(drive.driveId);
  expect(onchain).toBeTruthy();
  const onchainName = onchain!.name ? new TextDecoder().decode(onchain!.name.asBytes()) : null;
  expect(onchainName).toBe(newName);
});

test.fixme("clear contents → directory listing empty + root_cid resets", async ({ localPage }) => {
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
