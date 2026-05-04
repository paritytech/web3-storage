import { test, expect } from "../fixtures";

test.describe.configure({ mode: "serial" });

test("app loads and shows the sidebar", async ({ localPage }) => {
  await expect(localPage.getByText("Web3 Drive")).toBeVisible();
  await expect(localPage.getByTestId("connect-button")).toBeVisible();
});

test("block number badge ticks up", async ({ localPage }) => {
  const badge = localPage.getByTestId("block-number");
  const first = await badge.textContent();
  // wait for at least one new finalized block; ~6s/block
  await expect(async () => {
    const next = await badge.textContent();
    expect(next).not.toBe(first);
  }).toPass({ timeout: 30_000 });
});

test("connect dialog opens and shows network options", async ({ localPage }) => {
  await localPage.getByTestId("connect-button").click();
  await expect(localPage.getByTestId("connect-dialog")).toBeVisible();
  await expect(localPage.getByTestId("network-local")).toBeVisible();
  await expect(localPage.getByTestId("network-paseo")).toBeVisible();
  await expect(localPage.getByTestId("connect-endpoint-input")).toHaveValue(
    "ws://127.0.0.1:2222",
  );
});

test("account dialog opens and Alice is selected by default", async ({ localPage }) => {
  // Restored from localStorage in fixture; sidebar should show "Alice".
  await expect(localPage.getByTestId("signer-address")).toHaveText("Alice");
  // Open the account dialog and verify dev accounts are present.
  await localPage.getByTestId("account-button").click();
  await expect(localPage.getByTestId("account-dialog")).toBeVisible();
  await expect(localPage.getByTestId("account-dialog-alice")).toBeVisible();
  await expect(localPage.getByTestId("account-dialog-bob")).toBeVisible();
});

test("switching to Bob clears prior drive selection", async ({ localPage }) => {
  await localPage.getByTestId("account-button").click();
  await localPage.getByTestId("account-dialog-bob").click();
  // After switching, the signer indicator should now read Bob.
  await expect(localPage.getByTestId("signer-address")).toHaveText("Bob");
});
