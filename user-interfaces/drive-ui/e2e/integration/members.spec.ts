/**
 * Members specs.
 *
 * Verify the ManageAccessDialog handles SS58 validation, duplicate-member
 * detection, and adds a Reader whose row appears in the members table after
 * the tx settles (no manual refresh).
 */
import { test, expect } from "../fixtures";
import {
  Bob,
  Charlie,
  cleanupDrives,
  createDriveViaApi,
} from "@web3-storage/test-helpers";

test.describe.configure({ mode: "serial" });
// Each test creates a drive via the api helper (~30s incl. provider auto-
// accept) before touching the UI. Plus the actual UI tx (signAndSubmit
// waits for finalization, ~24-40s on local zombienet). Plus reload +
// dialog open + fill + click. 150s past the 120s wall the slow case bumps.
test.setTimeout(150_000);

test.afterEach(async () => {
  await cleanupDrives(Bob);
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
  const drive = await createDriveViaApi(Bob, {
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
  const drive = await createDriveViaApi(Bob, {
    name: `members-dup-${Date.now()}`,
    maxCapacity: 10_000_000n,
    storagePeriod: 10_000,
    payment: 120_000_000_000_000_000n,
    minProviders: 1,
  });
  await localPage.reload();

  await openAccessDialog(localPage, drive.driveId);
  // Bob is the owner and an implicit Admin member.
  await localPage.getByTestId("add-member-address").fill(Bob.address);
  await expect(localPage.getByTestId("add-member-error")).toContainText(/already.*member/i);
  await expect(localPage.getByTestId("add-member-submit")).toBeDisabled();
});

test("add Reader → list refreshes without manual click", async ({ localPage }) => {
  const drive = await createDriveViaApi(Bob, {
    name: `members-add-${Date.now()}`,
    maxCapacity: 10_000_000n,
    storagePeriod: 10_000,
    payment: 120_000_000_000_000_000n,
    minProviders: 1,
  });
  await localPage.reload();

  await openAccessDialog(localPage, drive.driveId);
  await localPage.getByTestId("add-member-address").fill(Charlie.address);
  await localPage.getByTestId("add-member-role").selectOption("Reader");
  await localPage.getByTestId("add-member-submit").click();

  // After tx settles, Charlie's row should appear without clicking refresh.
  // signAndSubmit waits for finalization (~24s on local zombienet) and the
  // refresh follows. 45s is comfortable headroom.
  await expect(localPage.getByTestId(`member-row-${Charlie.address}`)).toBeVisible({
    timeout: 45_000,
  });
});
