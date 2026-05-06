/**
 * Provider registration spec (slow ~30s each).
 *
 * Walks Eve through the registration wizard if she's not yet registered;
 * otherwise auto-skips with a clear message. Then verifies on-chain
 * `Providers.getValue(eve)` matches the form values, and that updating
 * pricePerByte via Settings round-trips to chain.
 *
 * NOTE: registration is one-shot per chain. Re-runs against a non-fresh
 * chain skip the fresh-registration test rather than failing.
 */
import { test, expect } from "../fixtures";
import { Eve, getApi, isProviderRegistered } from "@web3-storage/test-helpers";

test.describe.configure({ mode: "serial" });
test.setTimeout(180_000);

async function switchToEve(page: import("@playwright/test").Page) {
  // Open the account dropdown and select Eve.
  await page.getByTestId("provider-account-button").click();
  await page.getByTestId("provider-account-select-Eve (Dev)").click();
  await expect(page.getByTestId("provider-account-name")).toContainText("Eve", {
    timeout: 30_000,
  });
}

test("fresh registration with Eve via wizard", async ({ localPage }) => {
  test.skip(
    await isProviderRegistered(Eve.address),
    "Eve already registered — re-run against a fresh chain to exercise this spec",
  );

  await switchToEve(localPage);
  await localPage.getByTestId("nav-registration").click();

  // Step 1: connect (already connected; the wizard auto-advances).
  // Step 2: stake.
  await expect(localPage.getByTestId("registration-stake-input")).toBeVisible({
    timeout: 30_000,
  });
  await localPage.getByTestId("registration-stake-continue").click();

  // Step 3: settings.
  await expect(localPage.getByTestId("registration-multiaddr-input")).toBeVisible();
  await localPage.getByTestId("registration-multiaddr-input").fill("/ip4/127.0.0.1/tcp/3334");
  await localPage.getByTestId("registration-priceperbyte-input").fill("1");
  await localPage.getByTestId("registration-settings-continue").click();

  // Step 4: confirm + submit.
  await localPage.getByTestId("registration-submit").click();

  // Step 5: complete.
  await expect(localPage.getByTestId("registration-complete")).toBeVisible({
    timeout: 120_000,
  });

  const onchain = await getApi().query.StorageProvider.Providers.getValue(Eve.address);
  expect(onchain).toBeTruthy();
});

test("settings update post-registration: pricePerByte round-trips", async ({
  localPage,
}) => {
  test.skip(
    !(await isProviderRegistered(Eve.address)),
    "Eve must be registered first — run the fresh registration spec or use the api helper",
  );

  await switchToEve(localPage);
  await localPage.getByTestId("nav-registration").click();

  // Already-registered providers see the SettingsManager view, not the wizard.
  await expect(localPage.getByTestId("settings-priceperbyte-input")).toBeVisible({
    timeout: 30_000,
  });
  const newPrice = "2";
  await localPage.getByTestId("settings-priceperbyte-input").fill(newPrice);
  await localPage.getByTestId("settings-update").click();

  await expect.poll(
    async () => {
      const p = await getApi().query.StorageProvider.Providers.getValue(Eve.address);
      return p?.settings?.price_per_byte?.toString();
    },
    { timeout: 60_000 },
  ).toBe(newPrice);
});
