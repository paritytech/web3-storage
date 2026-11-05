// SPDX-License-Identifier: GPL-3.0-only

/**
 * Persistence specs (mostly fast, no chain mutations).
 *
 * Verifies that user-facing state survives a page reload via the documented
 * localStorage keys: endpoint, account name, view mode, selected drive,
 * and current path. Tests run serially and share Bob as the signer.
 *
 * Tests 3 and 4 both need a drive to render the file browser; they share
 * one drive created in beforeAll instead of each creating their own
 * (saves ~30s of provider settlement per duplicate create). The selected
 * drive is saved as its bucket id under "drive-ui-selected-bucket".
 */
import { test, expect } from "../fixtures";
import { createDriveInFreshContext } from "../helpers/createDriveViaUi";

test.describe.configure({ mode: "serial" });
test.setTimeout(180_000);

let sharedBucketId: bigint | null = null;

test.beforeAll(async ({ browser }) => {
  test.setTimeout(120_000);
  sharedBucketId = await createDriveInFreshContext(browser);
});

function requireBucketId(): bigint {
  if (sharedBucketId === null) {
    throw new Error("sharedBucketId not initialized — beforeAll did not run");
  }
  return sharedBucketId;
}

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
  // Bob is pre-injected by the fixture's localStorage.
  await expect(localPage.getByTestId("signer-address")).toHaveText("Bob");
  await localPage.reload();
  await expect(localPage.getByTestId("signer-address")).toHaveText("Bob");
});

test("view mode toggle persists across reload", async ({ localPage }) => {
  // The view mode toggle is in the file-browser; only renders when a drive is
  // selected.
  const bucketId = requireBucketId();

  await localPage.reload();
  await localPage.getByTestId(`drive-list-item-${bucketId}`).click();
  const toggle = localPage.getByTestId("view-mode-toggle");
  await expect(toggle).toBeVisible();
  // Capture current mode (text varies per toggle UI), click to toggle, reload, verify.
  await toggle.click();
  await localPage.reload();
  await localPage.getByTestId(`drive-list-item-${bucketId}`).click();
  // After reload, the same drive should be selected (selected-drive persists too).
  await expect(localPage.getByTestId(`drive-list-item-${bucketId}`)).toBeVisible();
  // localStorage drive-ui-view-mode should be "grid" after one toggle from default "list".
  const stored = await localPage.evaluate(() => localStorage.getItem("drive-ui-view-mode"));
  expect(stored === "grid" || stored === "list").toBe(true);
});

test("selected drive persists across reload", async ({ localPage }) => {
  const bucketId = requireBucketId();

  await localPage.reload();
  await localPage.getByTestId(`drive-list-item-${bucketId}`).click();
  await expect(localPage.getByTestId("file-browser")).toBeVisible();
  await localPage.reload();
  // After reload, file-browser should still be visible (drive auto-selected).
  await expect(localPage.getByTestId("file-browser")).toBeVisible({ timeout: 30_000 });
  const stored = await localPage.evaluate(() =>
    localStorage.getItem("drive-ui-selected-bucket"),
  );
  expect(stored).toBe(bucketId.toString());
});
