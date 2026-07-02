// SPDX-License-Identifier: GPL-3.0-only

/**
 * Members specs.
 *
 * Verify the ManageAccessDialog handles SS58 validation, duplicate-member
 * detection, and adds a Reader whose row appears in the members table after
 * the tx settles (no manual refresh).
 *
 * All three tests share a single drive created in beforeAll. Tests 1 + 2
 * are pure UI assertions on the dialog; test 3 mutates members (adds
 * Charlie) — declaration order is 1 → 2 → 3 in serial mode so Bob is
 * still the sole member when the duplicate-check test runs.
 */
import { test, expect } from "../fixtures";
import { Bob, Charlie, cleanupDrives } from "@web3-storage/test-helpers";
import { createDriveInFreshContext } from "../helpers/createDriveViaUi";

test.describe.configure({ mode: "serial" });
test.setTimeout(180_000);

let driveId: bigint;

test.beforeAll(async ({ browser }) => {
  test.setTimeout(180_000);
  driveId = await createDriveInFreshContext(browser, `members-${Date.now()}`);
});

test.afterAll(async () => {
  test.setTimeout(60_000);
  await cleanupDrives(Bob);
});

async function openAccessDialog(page: import("@playwright/test").Page) {
  await page.reload();
  await page.getByTestId(`drive-list-item-${driveId}`).hover();
  await page.getByTestId(`drive-list-access-${driveId}`).click();
  await expect(page.getByTestId("manage-access-dialog")).toBeVisible();
}

test("SS58 validation rejects garbage input", async ({ localPage }) => {
  await openAccessDialog(localPage);
  // ManageAccessDialog updates `validationError` synchronously on input change
  // and disables the submit button — there's nothing to click here, just
  // assert the inline error appears.
  await localPage.getByTestId("add-member-address").fill("not-an-address");
  await expect(localPage.getByTestId("add-member-error")).toContainText(/ss58|invalid/i);
  await expect(localPage.getByTestId("add-member-submit")).toBeDisabled();
});

test("duplicate-member check rejects already-member address", async ({ localPage }) => {
  await openAccessDialog(localPage);
  // Bob is the owner and an implicit Admin member.
  await localPage.getByTestId("add-member-address").fill(Bob.address);
  await expect(localPage.getByTestId("add-member-error")).toContainText(/already.*member/i);
  await expect(localPage.getByTestId("add-member-submit")).toBeDisabled();
});

test("add Reader → list refreshes without manual click", async ({ localPage }) => {
  await openAccessDialog(localPage);
  await localPage.getByTestId("add-member-address").fill(Charlie.address);
  await localPage.getByTestId("add-member-role").selectOption("Reader");
  await localPage.getByTestId("add-member-submit").click();

  // After tx settles, Charlie's row should appear without clicking refresh.
  // signAndSubmit waits for finalization (~24-36s on local zombienet) and
  // refresh follows. 45s used to be "comfortable headroom" but CI runs the
  // drive-ui suite back-to-back with file-ops which holds the chain busy
  // for several minutes, and the set_member finalize has overshot 45s
  // there. Match the 90s convention used by realtime.spec.ts for similar
  // post-tx UI assertions.
  await expect(localPage.getByTestId(`member-row-${Charlie.address}`)).toBeVisible({
    timeout: 90_000,
  });
});
