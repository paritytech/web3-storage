/**
 * Accounts spec.
 *
 * 1) Switch active account from Alice to Bob.
 * 2) Add a custom seed (`//Eve`) → derives the address and appears in the list.
 *
 * Note: console-ui auto-selects Alice on load via useStorage; persistence
 * across reload is a PR 4 item.
 */
import { test, expect } from "../fixtures";

test.describe.configure({ mode: "serial" });
test.setTimeout(120_000);

test("switch active account: Alice → Bob", async ({ localPage }) => {
  await localPage.getByTestId("nav-accounts").click();
  await expect(localPage.getByTestId("accounts-list")).toBeVisible();

  // Default active should be Alice (auto-set on local).
  await expect(localPage.getByTestId("accounts-active-badge-Alice")).toBeVisible({
    timeout: 30_000,
  });

  await localPage.getByTestId("accounts-set-active-Bob").click();
  await expect(localPage.getByTestId("accounts-active-badge-Bob")).toBeVisible({
    timeout: 30_000,
  });
  // Sidebar signer-name reflects the new account.
  await expect(localPage.getByTestId("signer-name")).toHaveText("Bob");
});

test("add custom seed //Eve → derives address + appears in list", async ({ localPage }) => {
  await localPage.getByTestId("nav-accounts").click();
  await expect(localPage.getByTestId("accounts-custom-form")).toBeVisible();

  const customName = `Eve-${Date.now()}`;
  await localPage.getByTestId("accounts-custom-name-input").fill(customName);
  await localPage.getByTestId("accounts-custom-seed-input").fill("//Eve");
  await localPage.getByTestId("accounts-custom-submit").click();

  await expect(localPage.getByTestId(`accounts-list-row-${customName}`)).toBeVisible({
    timeout: 30_000,
  });
});
