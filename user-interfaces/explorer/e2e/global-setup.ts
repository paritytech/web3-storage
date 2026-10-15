// SPDX-License-Identifier: GPL-3.0-only

/**
 * Playwright globalSetup — runs once before any spec.
 *
 * The explorer renders whatever the network holds, so the only setup is
 * making the chain non-empty: Alice registered as a provider guarantees the
 * provider-count and total-stake summary stats are provably non-zero.
 * `registerProviderViaApi` is idempotent, and nothing is cleaned up — the
 * explorer must tolerate whatever state other suites left behind.
 */
import { Alice, registerProviderViaApi, disconnectApi } from "@web3-storage/test-helpers";

export default async function globalSetup() {
  await registerProviderViaApi(Alice);
  disconnectApi();
}
