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

test("provider header shows wallet area", async ({ localPage }) => {
  // Provider auto-connects dev accounts (Alice) when in dev mode.
  // The header should render the navigation links.
  await expect(localPage.locator("nav").first()).toBeVisible();
});
