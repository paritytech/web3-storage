import { makeLocalPageFixture, expect } from "@web3-storage/test-helpers";

export const test = makeLocalPageFixture({
  localStorage: {
    "web3-storage-selected-network": "local",
    "provider-dashboard-wallet-mode": "dev",
  },
});

export { expect };
