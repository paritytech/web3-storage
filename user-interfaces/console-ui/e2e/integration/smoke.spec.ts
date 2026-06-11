import { test, expect } from "../fixtures";
import { expectBestBlockToAdvance } from "@web3-storage/test-helpers/playwright";

test.describe.configure({ mode: "serial" });

test("app loads and connects to local chain", async ({ localPage }) => {
  await expect(localPage.getByTestId("block-number")).toBeVisible();
});

test("chain produces blocks (best-block liveness)", async ({ localPage }) => {
  await expectBestBlockToAdvance(localPage);
});

test("dashboard navigation links are visible", async ({ localPage }) => {
  // Console-ui has navigation across Dashboard/Storage/Explorer/Accounts.
  // Verify the main nav is rendered (not hidden behind a connection error).
  await expect(localPage.locator("nav").first()).toBeVisible();
});
