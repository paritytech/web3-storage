// SPDX-License-Identifier: Apache-2.0

import { Eve } from "@web3-storage/test-helpers";
import { makeLocalPageFixture, expect } from "@web3-storage/test-helpers/playwright";

export const test = makeLocalPageFixture({
  localStorage: {
    "web3-storage-selected-network": "local",
    "provider-dashboard-wallet-mode": "dev",
    // Pre-select Eve as the active account so specs can assume she's
    // signed in. globalSetup pre-registers Eve as a provider; specs that
    // need a different account (e.g. the wizard test using Ferdie) switch
    // explicitly via the dropdown.
    "provider-dashboard-wallet-account": Eve.address,
  },
});

export { expect };
