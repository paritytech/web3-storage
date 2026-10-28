// SPDX-License-Identifier: GPL-3.0-only

/**
 * Real-time event subscription specs (multi-tab).
 *
 * Open two browser contexts, change chain state from the test, assert both
 * tabs' sidebars reflect the change without manual refresh. Exercises the
 * StorageProvider.{BucketCreated,MemberSet,MemberRemoved} subscription.
 */
import { test, expect } from "../fixtures";
import { removeMember, setMember } from "@web3-storage/sdk";
import { Bob, Charlie, createDriveViaApi, getApi } from "@web3-storage/test-helpers";
import {
  waitForConnection,
  waitForMinBlock,
} from "@web3-storage/test-helpers/playwright";

test.describe.configure({ mode: "serial" });
test.setTimeout(180_000);

async function openTabB(browser: import("@playwright/test").Browser) {
  // New context with the same localStorage seed as the fixture.
  const ctx = await browser.newContext();
  await ctx.addInitScript(() => {
    localStorage.setItem("web3-storage-selected-network", "local");
    localStorage.setItem("drive-ui-account-name", "Bob");
  });
  const tabB = await ctx.newPage();
  await tabB.goto("http://localhost:5174/");
  // Tab B has its own chain WS — wait until it's actually subscribed and has
  // observed a finalized block before asserting cross-tab event propagation.
  await waitForConnection(tabB, 60_000);
  await waitForMinBlock(tabB, 3, 60_000);
  return { tabB, ctx };
}

test("BucketCreated cross-tab", async ({ localPage, browser }) => {
  const { tabB, ctx } = await openTabB(browser);
  try {
    const { bucketId } = await createDriveViaApi(Bob, {
      maxCapacity: 10_000_000n,
      storagePeriod: 10_000,
    });

    // Tab A also reflects (sanity).
    await expect(localPage.getByTestId(`drive-list-item-${bucketId}`)).toBeVisible({
      timeout: 90_000,
    });
    // Tab B reflects without reload.
    await expect(tabB.getByTestId(`drive-list-item-${bucketId}`)).toBeVisible({
      timeout: 90_000,
    });
  } finally {
    await ctx.close();
  }
});

test("MemberSet and MemberRemoved cross-tab", async ({ localPage, browser }) => {
  const { tabB, ctx } = await openTabB(browser);
  try {
    // Charlie owns the bucket; adding Bob as a member must make it appear in
    // Bob's list, and removing him must make it disappear.
    const { bucketId } = await createDriveViaApi(Charlie, {
      maxCapacity: 10_000_000n,
      storagePeriod: 10_000,
    });

    await setMember(getApi(), Charlie, bucketId, Bob, "Reader");
    await expect(tabB.getByTestId(`drive-list-item-${bucketId}`)).toBeVisible({
      timeout: 90_000,
    });
    await expect(localPage.getByTestId(`drive-list-item-${bucketId}`)).toBeVisible({
      timeout: 90_000,
    });

    await removeMember(getApi(), Charlie, bucketId, Bob);
    await expect(tabB.getByTestId(`drive-list-item-${bucketId}`)).toBeHidden({
      timeout: 90_000,
    });
    await expect(localPage.getByTestId(`drive-list-item-${bucketId}`)).toBeHidden({
      timeout: 90_000,
    });
  } finally {
    await ctx.close();
  }
});
