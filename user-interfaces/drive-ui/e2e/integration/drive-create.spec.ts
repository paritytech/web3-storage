/**
 * Create-drive specs (slow ~30–90s each due to provider acceptance).
 *
 * Each test fills the new-drive form, picks a commit-strategy variant in the
 * UI, submits, and verifies the drive appears on chain with the right name.
 *
 * The runtime no longer round-trips `commit_strategy` (the file-system pallet
 * was simplified — see `user-interfaces/PAPI_OVERHAUL.md`), so we only assert
 * what's still observable: the drive exists with the expected name. Until
 * drive-ui catches up, the UI lets the user pick a strategy that the chain
 * silently ignores; reflecting that in the spec keeps the test honest.
 */
import { test, expect } from "../fixtures";
import {
  Alice,
  cleanupDrives,
  registerProviderViaApi,
  getApi,
} from "@web3-storage/test-helpers";

test.describe.configure({ mode: "serial" });
test.setTimeout(180_000);

test.beforeAll(async () => {
  test.setTimeout(120_000);
  // Make sure Alice is registered as a provider so agreement requests
  // emitted by drive creation can be auto-accepted by the local provider node
  // (which runs as `//Alice` per `just start-provider`'s default).
  await registerProviderViaApi(Alice);
});

test.afterEach(async () => {
  await cleanupDrives(Alice);
});

async function fillBaseFields(page: import("@playwright/test").Page, name: string) {
  await page.getByTestId("new-drive-button").click();
  await expect(page.getByTestId("new-drive-dialog")).toBeVisible();
  await page.getByTestId("new-drive-name").fill(name);
  // Capacity / duration / payment / min-providers — defaults are fine for these tests.
}

/**
 * Wait for a freshly-created drive to land in Alice's UserDrives. Returns
 * the latest drive id. Fresh chain's first drive has id 0n which is falsy
 * in JS — poll on `length`, then read the id outside the poll.
 */
async function waitForCreatedDriveId(): Promise<bigint> {
  const api = getApi();
  await expect.poll(
    async () => {
      const ids = await api.query.DriveRegistry.UserDrives.getValue(Alice.address);
      return ids?.length ?? 0;
    },
    { timeout: 120_000, intervals: [1000, 2000, 3000] },
  ).toBeGreaterThan(0);
  const ids = await api.query.DriveRegistry.UserDrives.getValue(Alice.address);
  return BigInt(ids![ids!.length - 1]);
}

async function expectDriveOnChain(driveId: bigint, expectedName: string) {
  const drive = await getApi().query.DriveRegistry.Drives.getValue(driveId);
  expect(drive).toBeTruthy();
  // `drive.name` decodes to a Uint8Array (Option<BoundedVec<u8, _>> on chain).
  const onchainName = drive?.name ? new TextDecoder().decode(drive.name) : null;
  expect(onchainName).toBe(expectedName);
}

test("Immediate strategy: drive lands on chain", async ({ localPage }) => {
  const name = `immediate-${Date.now()}`;
  await fillBaseFields(localPage, name);
  await localPage.getByTestId("commit-strategy-immediate").click();
  await localPage.getByTestId("new-drive-submit").click();

  const driveId = await waitForCreatedDriveId();
  await expectDriveOnChain(driveId, name);
});

test("Batched(50) strategy: drive lands on chain", async ({ localPage }) => {
  const name = `batched-${Date.now()}`;
  await fillBaseFields(localPage, name);
  await localPage.getByTestId("commit-strategy-batched").click();
  await localPage.getByTestId("commit-strategy-batched-interval").fill("50");
  await localPage.getByTestId("new-drive-submit").click();

  const driveId = await waitForCreatedDriveId();
  await expectDriveOnChain(driveId, name);
});

test("Manual strategy: drive lands on chain", async ({ localPage }) => {
  const name = `manual-${Date.now()}`;
  await fillBaseFields(localPage, name);
  await localPage.getByTestId("commit-strategy-manual").click();
  await localPage.getByTestId("new-drive-submit").click();

  const driveId = await waitForCreatedDriveId();
  await expectDriveOnChain(driveId, name);
});
