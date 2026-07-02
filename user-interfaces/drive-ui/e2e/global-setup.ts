// SPDX-License-Identifier: Apache-2.0

/**
 * Playwright globalSetup — runs once before any spec.
 *
 * Ensures the provider registry holds ONLY Alice (the auto-coordinator's
 * account) when drive-ui tests run. Drives the runtime's provider selection
 * (`available_primary_providers[0]`) deterministically — without this, prior
 * suites' leftover registrations (Eve from provider-e2e, Ferdie from the
 * registration wizard test) can win the lottery and the agreement request
 * sits unaccepted.
 */
import {
  Alice,
  cleanProviderRegistry,
  registerProviderViaApi,
  disconnectApi,
} from "@web3-storage/test-helpers";

export default async function globalSetup() {
  await cleanProviderRegistry([Alice]);
  await registerProviderViaApi(Alice);
  disconnectApi();
}
