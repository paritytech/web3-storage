// SPDX-License-Identifier: GPL-3.0-only

/**
 * Create-drive spec (slow ~30–90s due to provider acceptance).
 *
 * Walks the new-drive form, submits, and verifies the new bucket exists on
 * chain with Bob as its admin and appears in the drive list. The chain has
 * no bucket deletion, so the bucket stays after the test.
 */
import { test, expect } from "../fixtures";
import { isSameAddress } from "@web3-storage/sdk";
import { Bob, getApi } from "@web3-storage/test-helpers";
import { createDriveViaUi } from "../helpers/createDriveViaUi";

test.describe.configure({ mode: "serial" });
test.setTimeout(180_000);

// Provider registration happens once in playwright globalSetup. Don't
// re-register from per-spec beforeAll — that submits an extra Alice tx
// per spec and races the provider node's auto-coordinator on Alice's
// nonce, which then refuses to accept_agreement for our drives.

test("created bucket lands on chain with Bob as admin", async ({ localPage }) => {
  // globalSetup registered Alice as the only provider, so the first picker
  // row is the one we want.
  const bucketId = await createDriveViaUi(localPage);

  const bucket = await getApi().query.StorageProvider.Buckets.getValue(bucketId);
  expect(bucket).toBeTruthy();
  const bob = bucket?.members.find((m) => isSameAddress(m.account, Bob.address));
  expect(bob?.role.type).toBe("Admin");

  await expect(localPage.getByTestId(`drive-list-item-${bucketId}`)).toBeVisible({
    timeout: 90_000,
  });
  await expect(localPage.getByTestId(`drive-list-item-${bucketId}`)).toContainText(
    `Bucket #${bucketId}`,
  );
});
