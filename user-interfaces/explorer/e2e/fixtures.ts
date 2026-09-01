// SPDX-License-Identifier: GPL-3.0-only

import { makeLocalPageFixture, expect } from "@web3-storage/test-helpers/playwright";

// Read-only app: only the network selection is injected — no wallet keys.
export const test = makeLocalPageFixture({
  localStorage: {
    "web3-storage-selected-network": "local",
  },
});

export { expect };
