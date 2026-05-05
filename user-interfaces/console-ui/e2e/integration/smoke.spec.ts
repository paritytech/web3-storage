import { test, expect } from "../fixtures";

test.describe.configure({ mode: "serial" });

test("app loads and connects to local chain", async ({ localPage }) => {
  await expect(localPage.getByTestId("block-number")).toBeVisible();
});

test("block number ticks up over time", async ({ localPage }) => {
  const badge = localPage.getByTestId("block-number");
  const first = await badge.textContent();
  await expect(async () => {
    const next = await badge.textContent();
    expect(next).not.toBe(first);
  }).toPass({ timeout: 30_000 });
});

test("dashboard navigation links are visible", async ({ localPage }) => {
  // Console-ui has navigation across Dashboard/Storage/Explorer/Accounts.
  // Verify the main nav is rendered (not hidden behind a connection error).
  await expect(localPage.locator("nav").first()).toBeVisible();
});
