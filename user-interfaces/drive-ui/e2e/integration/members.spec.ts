/**
 * Members specs.
 *
 * Verify the ManageAccessDialog handles SS58 validation, duplicate-member
 * detection, and adds a Reader whose row appears in the members table after
 * the tx settles (no manual refresh).
 */
import { test, expect } from "../fixtures";
import {
  Alice,
  Bob,
  cleanupDrives,
  createDriveViaApi,
  registerProviderViaApi,
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

async function openAccessDialog(
  page: import("@playwright/test").Page,
  driveId: bigint,
) {
  await page.getByTestId(`drive-list-item-${driveId}`).hover();
  await page.getByTestId(`drive-list-access-${driveId}`).click();
  await expect(page.getByTestId("manage-access-dialog")).toBeVisible();
}

test("SS58 validation rejects garbage input", async ({ localPage }) => {
  const drive = await createDriveViaApi(Alice, {
    name: `members-ss58-${Date.now()}`,
    maxCapacity: 10_000_000n,
    storagePeriod: 10_000,
    payment: 120_000_000_000_000_000n,
    minProviders: 1,
  });
  await localPage.reload();

  await openAccessDialog(localPage, drive.driveId);
  // ManageAccessDialog updates `validationError` synchronously on input change
  // and disables the submit button — there's nothing to click here, just
  // assert the inline error appears.
  await localPage.getByTestId("add-member-address").fill("not-an-address");
  await expect(localPage.getByTestId("add-member-error")).toContainText(/ss58|invalid/i);
  await expect(localPage.getByTestId("add-member-submit")).toBeDisabled();
});

test("duplicate-member check rejects already-member address", async ({ localPage }) => {
  const drive = await createDriveViaApi(Alice, {
    name: `members-dup-${Date.now()}`,
    maxCapacity: 10_000_000n,
    storagePeriod: 10_000,
    payment: 120_000_000_000_000_000n,
    minProviders: 1,
  });
  await localPage.reload();

  await openAccessDialog(localPage, drive.driveId);
  // Alice is the owner and an implicit Admin member.
  await localPage.getByTestId("add-member-address").fill(Alice.address);
  await expect(localPage.getByTestId("add-member-error")).toContainText(/already.*member/i);
  await expect(localPage.getByTestId("add-member-submit")).toBeDisabled();
});

test("add Reader → list refreshes without manual click", async ({ localPage }) => {
  const drive = await createDriveViaApi(Alice, {
    name: `members-add-${Date.now()}`,
    maxCapacity: 10_000_000n,
    storagePeriod: 10_000,
    payment: 120_000_000_000_000_000n,
    minProviders: 1,
  });
  await localPage.reload();

  await openAccessDialog(localPage, drive.driveId);
  await localPage.getByTestId("add-member-address").fill(Bob.address);
  await localPage.getByTestId("add-member-role").selectOption("Reader");
  await localPage.getByTestId("add-member-submit").click();

  // After tx settles, Bob's row should appear without clicking refresh. The
  // ManageAccessDialog refresh hits chain immediately after handleAdd; on a
  // fresh CI chain the inBlock + refresh round-trip can run ~12-30s.
  await expect(localPage.getByTestId(`member-row-${Bob.address}`)).toBeVisible({
    timeout: 60_000,
  });
});
