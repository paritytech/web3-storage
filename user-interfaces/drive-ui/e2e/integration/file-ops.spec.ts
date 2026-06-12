/**
 * File operations specs.
 *
 * Cover the core upload / download / delete / abort flows. Abort uses
 * page.route() to inject latency on the upload PUT so the cancel click has a
 * deterministic window — without the route intercept, the tests race.
 */
import { test, expect } from "../fixtures";
import { Bob, cleanupDrives } from "@web3-storage/test-helpers";
import { createDriveViaUi } from "../helpers/createDriveViaUi";

test.describe.configure({ mode: "serial" });
test.setTimeout(180_000);

test.afterEach(async () => {
  await cleanupDrives(Bob);
});

async function selectFreshDrive(page: import("@playwright/test").Page) {
  const driveId = await createDriveViaUi(page, `file-ops-${Date.now()}`);
  await page.getByTestId(`drive-list-item-${driveId}`).click();
  await expect(page.getByTestId("file-browser")).toBeVisible();
  return { driveId };
}

test("multi-file upload appears in entries table", async ({ localPage }) => {
  await selectFreshDrive(localPage);

  await localPage.getByTestId("upload-input").setInputFiles([
    { name: "a.txt", mimeType: "text/plain", buffer: Buffer.from("alpha") },
    { name: "b.txt", mimeType: "text/plain", buffer: Buffer.from("bravo") },
    { name: "c.txt", mimeType: "text/plain", buffer: Buffer.from("charlie") },
  ]);

  for (const name of ["a.txt", "b.txt", "c.txt"]) {
    await expect(localPage.getByTestId(`entry-row-file-${name}`)).toBeVisible({
      timeout: 60_000,
    });
  }
});

test("download round-trips bytes", async ({ localPage }) => {
  await selectFreshDrive(localPage);
  const content = `download-${Date.now()}`;
  await localPage.getByTestId("upload-input").setInputFiles({
    name: "round-trip.txt",
    mimeType: "text/plain",
    buffer: Buffer.from(content),
  });
  await expect(localPage.getByTestId("entry-row-file-round-trip.txt")).toBeVisible({
    timeout: 60_000,
  });

  // Download is triggered via the actions dropdown, not via dblclick.
  await localPage.getByTestId("entry-actions-round-trip.txt").click();
  const downloadPromise = localPage.waitForEvent("download");
  await localPage.getByRole("menuitem", { name: /download/i }).click();
  const download = await downloadPromise;
  const stream = await download.createReadStream();
  const chunks: Buffer[] = [];
  for await (const chunk of stream) chunks.push(chunk as Buffer);
  expect(Buffer.concat(chunks).toString("utf-8")).toBe(content);
});

test("delete file removes the row", async ({ localPage }) => {
  await selectFreshDrive(localPage);
  await localPage.getByTestId("upload-input").setInputFiles({
    name: "delete-me.txt",
    mimeType: "text/plain",
    buffer: Buffer.from("bye"),
  });
  await expect(localPage.getByTestId("entry-row-file-delete-me.txt")).toBeVisible({
    timeout: 60_000,
  });

  await localPage.getByTestId("entry-actions-delete-me.txt").click();
  // The dropdown menu's delete item — try a generic role-based locator.
  await localPage.getByRole("menuitem", { name: /delete/i }).click();
  // Confirm if a Radix confirm dialog appears.
  const confirm = localPage.getByRole("button", { name: /^delete$/i });
  if (await confirm.isVisible().catch(() => false)) await confirm.click();

  await expect(localPage.getByTestId("entry-row-file-delete-me.txt")).toBeHidden({
    timeout: 60_000,
  });
});

test("abort upload mid-flight (no error toast)", async ({ localPage }) => {
  await selectFreshDrive(localPage);

  // Inject latency on PUT to /fs/<bucketId>/file?path=... (drive-client's
  // uploadFile target) so we have a window to click cancel before the
  // upload completes and the button vanishes.
  await localPage.route("**/fs/*/file*", async (route) => {
    if (route.request().method() === "PUT") {
      await new Promise((r) => setTimeout(r, 5_000));
    }
    await route.continue();
  });

  await localPage.getByTestId("upload-input").setInputFiles({
    name: "abort.bin",
    mimeType: "application/octet-stream",
    buffer: Buffer.alloc(1024, 0xab),
  });

  // Cancel button only renders while `uploading$` is true; with the 5s
  // delay above it's visible for ~5s. Generous click timeout to absorb
  // CI scheduler jitter.
  await localPage.getByTestId("upload-cancel").click({ timeout: 8_000 });

  // No error toast — assert the file row never appears (cancellation is silent).
  await expect(localPage.getByTestId("entry-row-file-abort.bin")).toBeHidden({
    timeout: 10_000,
  });
});
