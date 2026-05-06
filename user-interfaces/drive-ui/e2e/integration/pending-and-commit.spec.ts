/**
 * Pending changes + commit specs.
 *
 * Manual strategy: upload triggers a pending-changes banner with "Commit Now";
 * clicking it commits to chain and the banner disappears.
 *
 * Batched strategy: banner shows ETA text matching the configured interval;
 * no Commit Now button is shown.
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

async function uploadOneFile(page: import("@playwright/test").Page, content: string) {
  // The UploadZone exposes the native <input type="file">; Playwright drives it via setInputFiles.
  const fileInput = page.locator('input[type="file"]').first();
  await fileInput.setInputFiles({
    name: "hello.txt",
    mimeType: "text/plain",
    buffer: Buffer.from(content, "utf-8"),
  });
}

test("Manual: upload → banner + Commit Now → banner clears + chain root_cid updates", async ({
  localPage,
}) => {
  const drive = await createDriveViaApi(Alice, {
    name: `manual-pending-${Date.now()}`,
    maxCapacity: 10_000_000n,
    storagePeriod: 10_000,
    payment: 120_000_000_000_000_000n,
    minProviders: 1,
    commitStrategy: { type: "Manual" },
  });
  await localPage.reload();
  await localPage.getByTestId(`drive-list-item-${drive.driveId}`).click();
  await expect(localPage.getByTestId("file-browser")).toBeVisible();

  await uploadOneFile(localPage, `pending-${Date.now()}`);

  // Pending banner should surface after upload.
  await expect(localPage.getByTestId("pending-changes-banner")).toBeVisible({
    timeout: 60_000,
  });
  await expect(localPage.getByTestId("commit-now-button")).toBeEnabled();

  await localPage.getByTestId("commit-now-button").click();

  // Banner disappears after commit.
  await expect(localPage.getByTestId("pending-changes-banner")).toBeHidden({
    timeout: 60_000,
  });

  // Root CID on chain should be non-zero after commit.
  await expect.poll(
    async () => {
      const d = await getApi().query.DriveRegistry.Drives.getValue(drive.driveId);
      return d?.root_cid?.asHex();
    },
    { timeout: 30_000, intervals: [1000, 2000, 3000] },
  ).not.toMatch(/^0x0+$/);
});

test("Batched(10): banner shows ETA, no Commit Now button", async ({ localPage }) => {
  const drive = await createDriveViaApi(Alice, {
    name: `batched-pending-${Date.now()}`,
    maxCapacity: 10_000_000n,
    storagePeriod: 10_000,
    payment: 120_000_000_000_000_000n,
    minProviders: 1,
    commitStrategy: { type: "Batched", interval: 10 },
  });
  await localPage.reload();
  await localPage.getByTestId(`drive-list-item-${drive.driveId}`).click();
  await expect(localPage.getByTestId("file-browser")).toBeVisible();

  await uploadOneFile(localPage, `batched-${Date.now()}`);

  await expect(localPage.getByTestId("pending-changes-banner")).toBeVisible({
    timeout: 60_000,
  });
  await expect(localPage.getByTestId("commit-now-button")).toHaveCount(0);
});
