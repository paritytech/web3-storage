// SPDX-License-Identifier: GPL-3.0-only

/**
 * Drive-ui Playwright fixtures.
 *
 * Pre-injects the local network selection (ws://127.0.0.1:2222) and the
 * `Bob` dev account so each test starts from a known signed-in state.
 *
 * Bob (not Alice) is intentional: `just start-provider` runs the provider
 * node as `//Alice`, so any test-side tx that signs as Alice races with
 * the provider's auto-coordinator on Alice's nonce. Using Bob keeps the
 * test signer and the provider signer cleanly separated.
 */
import { makeLocalPageFixture, expect } from "@web3-storage/test-helpers";

export const test = makeLocalPageFixture({
  localStorage: {
    "web3-storage-selected-network": "local",
    "drive-ui-account-name": "Bob",
  },
});

export { expect };
