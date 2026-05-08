/**
 * Provider multiaddr update spec.
 *
 * Requires Eve to be registered. Updates the multiaddr field, submits, and
 * asserts on-chain `Providers.multiaddr` matches.
 */
import { test, expect } from "../fixtures";
import { Eve, getApi } from "@web3-storage/test-helpers";

test.describe.configure({ mode: "serial" });
test.setTimeout(120_000);

// Eve is pre-registered by globalSetup; fixture pre-injects her as the
// active account.

test("update multiaddr → on-chain matches", async ({ localPage }) => {
  await localPage.getByTestId("nav-registration").click();

  const newAddr = `/ip4/127.0.0.1/tcp/${4000 + Math.floor(Math.random() * 100)}`;
  await localPage.getByTestId("settings-multiaddr-input").fill(newAddr);
  await localPage.getByTestId("settings-multiaddr-update").click();

  await expect.poll(
    async () => {
      const p = await getApi().query.StorageProvider.Providers.getValue(Eve.address);
      if (!p) return null;
      return new TextDecoder().decode(p.multiaddr);
    },
    { timeout: 60_000 },
  ).toBe(newAddr);
});
