/**
 * Drive-ui Playwright fixtures.
 *
 * Pre-injects the local network selection (ws://127.0.0.1:2222) and the
 * `Alice` dev account so each test starts from a known signed-in state.
 */
import { makeLocalPageFixture, expect } from "@web3-storage/test-helpers";

export const test = makeLocalPageFixture({
  localStorage: {
    "web3-storage-selected-network": "local",
    "drive-ui-account-name": "Alice",
  },
});

export { expect };
