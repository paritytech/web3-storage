/**
 * Provider registration spec (slow ~30s each).
 *
 * Wizard test uses Ferdie; globalSetup attempts to deregister Ferdie via
 * `cleanProviderRegistry` so this test exercises the fresh-registration
 * flow. The pallet's deregister is two-step (announce now, complete after
 * `DeregisterAnnouncementPeriod` = 48h), so on a chain that has already
 * seen a successful wizard run, Ferdie's `Providers` entry persists and
 * `register_provider` would reject with `ProviderAlreadyRegistered`. We
 * skip in that case rather than fail — re-runnability needs a fresh
 * chain. Settings test uses Eve, who's pre-registered by globalSetup.
 */
import { test, expect } from "../fixtures";
import { firstMatch, READ_OPTS } from "@web3-storage/sdk";
import { Eve, Ferdie, getApi } from "@web3-storage/test-helpers";

test.describe.configure({ mode: "serial" });
test.setTimeout(180_000);

async function switchToFerdie(page: import("@playwright/test").Page) {
  await page.getByTestId("provider-account-button").click();
  await page.getByTestId("provider-account-select-Ferdie (Dev)").click();
  await expect(page.getByTestId("provider-account-name")).toContainText("Ferdie", {
    timeout: 30_000,
  });
}

test("fresh registration with Ferdie via wizard", async ({ localPage }) => {
  const existing = await getApi().query.StorageProvider.Providers.getValue(Ferdie.address);
  test.skip(
    existing !== undefined,
    "Ferdie is already registered on chain (likely from a prior wizard run); the runtime's two-step deregister can't fully remove the entry within a 48h test window. Restart the chain to re-run.",
  );

  await switchToFerdie(localPage);
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

  // After registration, the wizard redirects to SettingsManager (there's no
  // dedicated "complete" step — the Registered badge in the settings header
  // is the only "you're registered" notification the UI exposes).
  //
  // Budget: `submitRegisterProvider` submits two sequential extrinsics
  // (`register_provider` + `update_provider_settings`) and awaits
  // finalization on each (~24–36s per tx on a busy parachain), then
  // `loadProviderData` runs another batch of queries before SettingsManager
  // unblocks. Locally that's ~72s; on the CI parity-large runner it has
  // run past 120s. 150s sits comfortably inside the 180s spec budget while
  // absorbing the worst case.
  await expect(localPage.getByTestId("provider-registered-badge")).toBeVisible({
    timeout: 150_000,
  });

  const onchain = await getApi().query.StorageProvider.Providers.getValue(Ferdie.address);
  expect(onchain).toBeTruthy();
});

test("settings update post-registration: pricePerByte round-trips", async ({
  localPage,
}) => {
  // Eve is pre-registered by globalSetup and the active account via
  // fixture localStorage; just navigate.
  await localPage.getByTestId("nav-registration").click();

  // Already-registered providers see the SettingsManager view, not the wizard.
  await expect(localPage.getByTestId("settings-priceperbyte-input")).toBeVisible({
    timeout: 30_000,
  });
  const newPrice = "2";
  await localPage.getByTestId("settings-priceperbyte-input").fill(newPrice);
  await localPage.getByTestId("settings-update").click();

  await firstMatch(
    getApi().query.StorageProvider.Providers.watchValue(Eve.address, READ_OPTS),
    ({ value }) => value?.settings?.price_per_byte?.toString() === newPrice,
    { timeoutMs: 60_000, description: `Eve's price_per_byte to become ${newPrice}` },
  );
});
