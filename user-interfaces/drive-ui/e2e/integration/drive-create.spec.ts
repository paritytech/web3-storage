/**
 * Create-drive specs (slow ~30–90s each due to provider acceptance).
 *
 * Each test fills the new-drive form, picks a commit-strategy variant, submits,
 * waits for the dialog to close on success, then verifies the on-chain
 * commit_strategy matches what the user picked.
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
 * Wait for the new-drive flow to land on chain — both UserDrives gets the
 * new id AND Drives.getValue(id) returns a fully-populated struct. The
 * dialog auto-closes after createDrive resolves, but UserDrives can
 * appear momentarily before Drives.commit_strategy is readable in the
 * same finalized view, so poll on the inner field rather than just on
 * length. (Fresh chain's first drive has id 0n which is falsy — the
 * `id !== null` check must be explicit.)
 */
async function waitForCreatedDriveId(): Promise<bigint> {
  const api = getApi();
  let id: bigint | null = null;
  await expect.poll(
    async () => {
      const ids = await api.query.DriveRegistry.UserDrives.getValue(Alice.address);
      if (!ids || ids.length === 0) return null;
      const candidate = BigInt(ids[ids.length - 1]);
      const drive = await api.query.DriveRegistry.Drives.getValue(candidate);
      const strat = (drive as { commit_strategy?: { type?: string } } | undefined)
        ?.commit_strategy?.type;
      if (strat) {
        id = candidate;
        return strat;
      }
      return null;
    },
    { timeout: 120_000, intervals: [1000, 2000, 3000] },
  ).not.toBeNull();
  return id!;
}

test("Immediate strategy round-trips to chain", async ({ localPage }) => {
  await fillBaseFields(localPage, `immediate-${Date.now()}`);
  await localPage.getByTestId("commit-strategy-immediate").click();
  await localPage.getByTestId("new-drive-submit").click();

  const driveId = await waitForCreatedDriveId();
  const drive = await getApi().query.DriveRegistry.Drives.getValue(driveId);
  expect(drive).toBeTruthy();
  expect(drive!.commit_strategy.type).toBe("Immediate");
});

test("Batched(50) strategy round-trips to chain", async ({ localPage }) => {
  await fillBaseFields(localPage, `batched-${Date.now()}`);
  await localPage.getByTestId("commit-strategy-batched").click();
  await localPage.getByTestId("commit-strategy-batched-interval").fill("50");
  await localPage.getByTestId("new-drive-submit").click();

  const driveId = await waitForCreatedDriveId();
  const drive = await getApi().query.DriveRegistry.Drives.getValue(driveId);
  expect(drive).toBeTruthy();
  expect(drive!.commit_strategy.type).toBe("Batched");
  expect(Number((drive!.commit_strategy as { value: { interval: number } }).value.interval)).toBe(50);
});

test("Manual strategy round-trips to chain", async ({ localPage }) => {
  await fillBaseFields(localPage, `manual-${Date.now()}`);
  await localPage.getByTestId("commit-strategy-manual").click();
  await localPage.getByTestId("new-drive-submit").click();

  const driveId = await waitForCreatedDriveId();
  const drive = await getApi().query.DriveRegistry.Drives.getValue(driveId);
  expect(drive).toBeTruthy();
  expect(drive!.commit_strategy.type).toBe("Manual");
});
