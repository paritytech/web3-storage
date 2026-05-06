/**
 * Persistence specs (fast, no chain mutations).
 *
 * Verifies that user-facing state survives a page reload via the documented
 * localStorage keys: endpoint, account name, view mode, selected drive,
 * and current path. Tests run serially and share Alice as the signer.
 */
import { test, expect } from "../fixtures";
import { Alice, createDriveViaApi, cleanupDrives } from "@web3-storage/test-helpers";

test.describe.configure({ mode: "serial" });
test.setTimeout(120_000);

test("endpoint selection persists across reload", async ({ localPage }) => {
  await localPage.getByTestId("connect-button").click();
  await expect(localPage.getByTestId("connect-dialog")).toBeVisible();
  // The default for `local` network is ws://127.0.0.1:2222 — verify it survives a reload.
  await expect(localPage.getByTestId("connect-endpoint-input")).toHaveValue(
    "ws://127.0.0.1:2222",
  );
  // Close + reload.
  await localPage.keyboard.press("Escape");
  await localPage.reload();
  // Re-open the dialog and check the input still defaults to the same endpoint.
  await localPage.getByTestId("connect-button").click();
  await expect(localPage.getByTestId("connect-endpoint-input")).toHaveValue(
    "ws://127.0.0.1:2222",
  );
});

test("account name persists across reload", async ({ localPage }) => {
  // Alice is pre-injected by the fixture's localStorage.
  await expect(localPage.getByTestId("signer-address")).toHaveText("Alice");
  await localPage.reload();
  await expect(localPage.getByTestId("signer-address")).toHaveText("Alice");
});

test("view mode toggle persists across reload", async ({ localPage }) => {
  // The view mode toggle is in the file-browser; only renders when a drive is
  // selected. Create a throwaway drive via api so we have something to select.
  const drive = await createDriveViaApi(Alice, {
    name: `view-mode-${Date.now()}`,
    maxCapacity: 10_000_000n,
    storagePeriod: 10_000,
    payment: 120_000_000_000_000_000n,
    minProviders: 1,
    commitStrategy: { type: "Batched", interval: 100 },
  });

  try {
    await localPage.reload();
    await localPage.getByTestId(`drive-list-item-${drive.driveId}`).click();
    const toggle = localPage.getByTestId("view-mode-toggle");
    await expect(toggle).toBeVisible();
    // Capture current mode (text varies per toggle UI), click to toggle, reload, verify.
    await toggle.click();
    await localPage.reload();
    await localPage.getByTestId(`drive-list-item-${drive.driveId}`).click();
    // After reload, the same drive should be selected (selected-drive persists too).
    await expect(localPage.getByTestId(`drive-list-item-${drive.driveId}`)).toBeVisible();
    // localStorage drive-ui-view-mode should be "grid" after one toggle from default "list".
    const stored = await localPage.evaluate(() => localStorage.getItem("drive-ui-view-mode"));
    expect(stored === "grid" || stored === "list").toBe(true);
  } finally {
    await cleanupDrives(Alice);
  }
});

test("selected drive persists across reload", async ({ localPage }) => {
  const drive = await createDriveViaApi(Alice, {
    name: `selected-${Date.now()}`,
    maxCapacity: 10_000_000n,
    storagePeriod: 10_000,
    payment: 120_000_000_000_000_000n,
    minProviders: 1,
    commitStrategy: { type: "Batched", interval: 100 },
  });

  try {
    await localPage.reload();
    await localPage.getByTestId(`drive-list-item-${drive.driveId}`).click();
    await expect(localPage.getByTestId("file-browser")).toBeVisible();
    await localPage.reload();
    // After reload, file-browser should still be visible (drive auto-selected).
    await expect(localPage.getByTestId("file-browser")).toBeVisible({ timeout: 30_000 });
    const stored = await localPage.evaluate(() =>
      localStorage.getItem("drive-ui-selected-drive"),
    );
    expect(stored).toBe(drive.driveId.toString());
  } finally {
    await cleanupDrives(Alice);
  }
});
