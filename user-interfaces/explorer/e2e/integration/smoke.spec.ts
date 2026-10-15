// SPDX-License-Identifier: GPL-3.0-only

import { test, expect } from "../fixtures";
import { expectBestBlockToAdvance } from "@web3-storage/test-helpers/playwright";

test.describe.configure({ mode: "serial" });

test("app loads and connects to local chain", async ({ localPage }) => {
  await expect(localPage.getByTestId("block-number")).toBeVisible();
});

test("chain produces blocks (best-block liveness)", async ({ localPage }) => {
  await expectBestBlockToAdvance(localPage);
});

test("summary stats render non-placeholder values", async ({ localPage }) => {
  // Each regex rejects the '…' loading placeholder, so toHaveText waits out
  // the snapshot load. Zeros format as real values ('0', '0 B'), keeping the
  // assertions valid on a bare chain — except providers/stake, which
  // globalSetup guarantees non-zero by registering Alice.
  const providers = localPage.getByTestId("summary-stat-providers");
  await expect(providers).toHaveText(/^[\d,]+$/, { timeout: 30_000 });
  const providerCount = Number((await providers.textContent())!.replace(/,/g, ""));
  expect(providerCount).toBeGreaterThanOrEqual(1);

  // Alice's stake >= 1000 tokens, so the value starts with a non-zero digit
  // and carries a token symbol (e.g. "1,000 UNIT").
  await expect(localPage.getByTestId("summary-stat-stake")).toHaveText(
    /^[1-9][\d,.]*(\s\S+)+$/
  );

  await expect(localPage.getByTestId("summary-stat-data")).toHaveText(
    /^[\d.,]+ (B|KB|MB|GB|TB|PB|EB)$/
  );

  await expect(localPage.getByTestId("summary-stat-agreements")).toHaveText(/^[\d,]+$/);
  await expect(localPage.getByTestId("summary-stat-buckets")).toHaveText(/^[\d,]+$/);
  await expect(localPage.getByTestId("summary-stat-challenges")).toHaveText(/^[\d,]+$/);
});

test("providers list shows the registered provider", async ({ localPage }) => {
  await localPage.getByTestId("nav-providers").click();
  await expect(localPage.getByTestId("providers-table")).toBeVisible({ timeout: 30_000 });
  const rows = localPage.locator('[data-testid^="providers-row-"]');
  expect(await rows.count()).toBeGreaterThanOrEqual(1);
});
