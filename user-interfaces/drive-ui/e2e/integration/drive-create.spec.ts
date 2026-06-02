/**
 * Create-drive spec (slow ~30–90s due to provider acceptance).
 *
 * Walks the new-drive form, submits, and verifies the drive appears on chain
 * with the right name. The runtime is the slim DriveInfo (8 fields), so the
 * `name` is the only user-supplied content we can round-trip.
 */
import { test, expect } from "../fixtures";
import {
  Bob,
  cleanupDrives,
  getApi,
} from "@web3-storage/test-helpers";

test.describe.configure({ mode: "serial" });
test.setTimeout(180_000);

// Provider registration happens once in playwright globalSetup. Don't
// re-register from per-spec beforeAll — that submits an extra Alice tx
// per spec and races the provider node's auto-coordinator on Alice's
// nonce, which then refuses to accept_agreement for our drives.

test.afterEach(async () => {
  await cleanupDrives(Bob);
});

async function fillBaseFields(page: import("@playwright/test").Page, name: string) {
  await page.getByTestId("new-drive-button").click();
  await expect(page.getByTestId("new-drive-dialog")).toBeVisible();
  await page.getByTestId("new-drive-name").fill(name);
  // Capacity / duration / price-per-byte — defaults are fine for these tests.
}

/** 
 * Wait for a freshly-created drive to land in Bob's UserDrives. Returns
 * the latest drive id. Fresh chain's first drive has id 0n which is falsy
 * in JS — poll on `length`, then read the id outside the poll.
 */
async function waitForCreatedDriveId(): Promise<bigint> {
  const api = getApi();
  await expect.poll(
    async () => {
      const ids = await api.query.DriveRegistry.UserDrives.getValue(Bob.address);
      return ids.length;
    },
    { timeout: 120_000, intervals: [1000, 2000, 3000] },
  ).toBeGreaterThan(0);
  const ids = await api.query.DriveRegistry.UserDrives.getValue(Bob.address);
  return ids[ids.length - 1];
}

async function expectDriveOnChain(driveId: bigint, expectedName: string) {
  const drive = await getApi().query.DriveRegistry.Drives.getValue(driveId);
  expect(drive).toBeTruthy();
  // `drive.name` decodes to a Uint8Array (Option<BoundedVec<u8, _>> on chain).
  const onchainName = drive?.name ? new TextDecoder().decode(drive.name) : null;
  expect(onchainName).toBe(expectedName);
}

test("drive lands on chain with the user-supplied name", async ({ localPage }) => {
  const name = `create-${Date.now()}`;
  await fillBaseFields(localPage, name);
  // Provider picker is embedded in the create dialog — picking a provider
  // IS the submit. globalSetup registered Alice as the lone provider, so
  // the first row is always the one we want.
  await expect(localPage.getByTestId("provider-picker")).toBeVisible({
    timeout: 30_000,
  });
  await localPage.getByTestId("provider-picker-select").first().click();

  const driveId = await waitForCreatedDriveId();
  await expectDriveOnChain(driveId, name);
});
