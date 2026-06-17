// SPDX-License-Identifier: GPL-3.0-only

import { test, expect } from "../fixtures";
import { expectBestBlockToAdvance } from "@web3-storage/test-helpers";

test.describe.configure({ mode: "serial" });

test("app loads and connects to local chain", async ({ localPage }) => {
  await expect(localPage.getByTestId("block-number")).toBeVisible();
});

test("chain produces blocks (best-block liveness)", async ({ localPage }) => {
  await expectBestBlockToAdvance(localPage);
});

test("provider header shows wallet area", async ({ localPage }) => {
  // Provider auto-connects dev accounts (Alice) when in dev mode.
  // The header should render the navigation links.
  await expect(localPage.locator("nav").first()).toBeVisible();
});
