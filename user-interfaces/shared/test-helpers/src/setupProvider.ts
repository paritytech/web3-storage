// SPDX-License-Identifier: GPL-3.0-only


import {
  Alice,
  cleanProviderRegistry,
  registerProviderViaApi,
  disconnectApi,
} from "@web3-storage/test-helpers";


async function globalSetupProvider() {
  await cleanProviderRegistry([Alice]);
  await registerProviderViaApi(Alice);
  disconnectApi();
}

globalSetupProvider().catch((err) => {
    console.error("Failed to set up provider with error:", err);
    process.exit(1);
});