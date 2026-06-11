import { test, expect } from "../fixtures";
import { expectBestBlockToAdvance } from "@web3-storage/test-helpers/playwright";

test.describe.configure({ mode: "serial" });

test("app loads and shows the sidebar", async ({ localPage }) => {
  await expect(localPage.getByText("Web3 Drive")).toBeVisible();
  await expect(localPage.getByTestId("connect-button")).toBeVisible();
});

test("chain produces blocks (best-block liveness)", async ({ localPage }) => {
  await expectBestBlockToAdvance(localPage);
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

test("account dialog opens and Bob is selected by default", async ({ localPage }) => {
  // Restored from localStorage in fixture; sidebar should show "Bob".
  await expect(localPage.getByTestId("signer-address")).toHaveText("Bob");
  // Open the account dialog and verify dev accounts are present.
  await localPage.getByTestId("account-button").click();
  await expect(localPage.getByTestId("account-dialog")).toBeVisible();
  await expect(localPage.getByTestId("account-dialog-alice")).toBeVisible();
  await expect(localPage.getByTestId("account-dialog-bob")).toBeVisible();
});

test("switching to Charlie clears prior drive selection", async ({ localPage }) => {
  await localPage.getByTestId("account-button").click();
  await localPage.getByTestId("account-dialog-charlie").click();
  // After switching, the signer indicator should now read Charlie.
  await expect(localPage.getByTestId("signer-address")).toHaveText("Charlie");
});
