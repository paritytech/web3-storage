// SPDX-License-Identifier: Apache-2.0

/**
 * Provider multiaddr update spec.
 *
 * Requires Eve to be registered. Updates the multiaddr field, submits, and
 * asserts on-chain `Providers.multiaddr` matches.
 */
import { firstMatch, READ_OPTS } from "@web3-storage/sdk";
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

  await firstMatch(
    getApi().query.StorageProvider.Providers.watchValue(Eve.address, READ_OPTS),
    ({ value }) => !!value && new TextDecoder().decode(value.multiaddr) === newAddr,
    { timeoutMs: 60_000, description: `Eve's multiaddr to become ${newAddr}` },
  );
});
