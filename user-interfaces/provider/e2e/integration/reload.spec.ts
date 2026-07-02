// SPDX-License-Identifier: Apache-2.0

/**
 * Provider reload-survival specs.
 *
 * Network selection + wallet account persist across reload. The fixture
 * pre-injects `provider-dashboard-wallet-mode: dev` and the local network;
 * these should still be in effect after reload.
 */
import { test, expect } from "../fixtures";

test.describe.configure({ mode: "serial" });
test.setTimeout(60_000);

test("network selection persists across reload", async ({ localPage }) => {
  await localPage.reload();
  // After reload we should still be connected to the local chain — block-number
  // becomes visible once a finalized block lands.
  await expect(localPage.getByTestId("block-number")).toBeVisible({ timeout: 60_000 });
});

test("wallet mode (dev) persists across reload", async ({ localPage }) => {
  await localPage.reload();
  await expect(localPage.getByTestId("block-number")).toBeVisible({ timeout: 60_000 });
  // Wallet-mode dev → account button shows a dev account name.
  await expect(localPage.getByTestId("provider-account-name")).toBeVisible({
    timeout: 30_000,
  });
  const stored = await localPage.evaluate(() =>
    localStorage.getItem("provider-dashboard-wallet-mode"),
  );
  expect(stored).toBe("dev");
});
