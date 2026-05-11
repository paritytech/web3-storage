/**
 * Bucket members spec (parallel to drive-ui's members.spec.ts).
 *
 * Uses console-ui's BucketInfoPanel instead of the drive-ui ManageAccessDialog.
 * Errors here are surfaced via toast (no inline error testid) so the
 * SS58/duplicate tests assert on toast text.
 */
import { test, expect } from "../fixtures";
import {
  Alice,
  Bob,
  cleanupBuckets,
  createBucketViaApi,
} from "@web3-storage/test-helpers";

test.describe.configure({ mode: "serial" });
test.setTimeout(180_000);

let bucketName: string;

test.beforeAll(async () => {
  test.setTimeout(120_000);
  bucketName = `members-${Date.now()}`;
  await createBucketViaApi(Alice, { name: bucketName });
});

test.afterAll(async () => {
  test.setTimeout(90_000);
  await cleanupBuckets(Alice);
});

test.beforeEach(async ({ localPage }) => {
  await localPage.getByTestId("nav-storage").click();
  await localPage.getByTestId("s3-bucket-selector").selectOption({ label: bucketName });
  // Expand the bucket info panel to surface members controls.
  await localPage.getByTestId("bucket-info-toggle").click();
  await expect(localPage.getByTestId("bucket-members-table")).toBeVisible({ timeout: 30_000 });
});

test("SS58 validation rejects garbage input", async ({ localPage }) => {
  await localPage.getByTestId("bucket-add-member-address").fill("not-an-address");
  await localPage.getByTestId("bucket-add-member-submit").click();
  // Surface error via toast.
  await expect(localPage.getByText(/failed to add|invalid|ss58/i).first()).toBeVisible({
    timeout: 30_000,
  });
});

test("duplicate-member check rejects already-member address", async ({ localPage }) => {
  // Alice is the owner and already a member.
  await localPage.getByTestId("bucket-add-member-address").fill(Alice.address);
  await localPage.getByTestId("bucket-add-member-submit").click();
  await expect(localPage.getByText(/already.*member|exists/i).first()).toBeVisible({
    timeout: 30_000,
  });
});

test("add Reader → row appears in members table", async ({ localPage }) => {
  await localPage.getByTestId("bucket-add-member-address").fill(Bob.address);
  await localPage.getByTestId("bucket-add-member-role").selectOption("Reader");
  await localPage.getByTestId("bucket-add-member-submit").click();

  await expect(localPage.getByTestId(`bucket-member-row-${Bob.address}`)).toBeVisible({
    timeout: 30_000,
  });
});
