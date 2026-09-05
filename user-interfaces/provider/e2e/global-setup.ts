// SPDX-License-Identifier: GPL-3.0-only

/**
 * Playwright globalSetup — runs once before any spec.
 *
 * Provider-ui tests need:
 *   - Alice registered (the provider node's auto-coordinator account, so
 *     bucket-creating helpers like `createBucketViaApi(Bob)` get their
 *     agreement requests accepted and don't pollute the chain with
 *     unaccepted requests for the next suite)
 *   - Eve registered with the test-fixed multiaddr — used by displays /
 *     multiaddr / registration's settings-update test as a pre-existing
 *     provider whose state the UI reads
 *   - Ferdie deregistered so the registration-wizard test can exercise
 *     the fresh-registration UI flow on every suite invocation
 *
 * `cleanProviderRegistry([Alice, Eve])` strips any other dev-account
 * providers (Bob/Charlie/Dave/Ferdie) that may have leaked from previous
 * runs — keeps the chain state deterministic across re-runs without a
 * full chain restart.
 */
import {
  Alice,
  Eve,
  cleanProviderRegistry,
  registerProviderViaApi,
  disconnectApi,
} from "@web3-storage/test-helpers";

export default async function globalSetup() {
  await cleanProviderRegistry([Alice, Eve]);
  await registerProviderViaApi(Alice);
  await registerProviderViaApi(Eve, { multiaddr: "/ip4/127.0.0.1/tcp/3334" });
  disconnectApi();
}
