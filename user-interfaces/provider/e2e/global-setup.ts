/**
 * Playwright globalSetup — runs once before any spec.
 *
 * Pre-registers Eve as a provider with the test-fixed multiaddr (used by
 * displays / multiaddr / registration's settings-update test).
 *
 * Deregisters Ferdie if she's currently registered, so the wizard test
 * (which exercises the fresh-registration UI flow) can run on every suite
 * invocation rather than only against a fresh chain. Deregistration is a
 * no-op when Ferdie isn't registered, and only fails if she has committed
 * bytes (= active agreements) — which she shouldn't, since the wizard
 * test only registers her without taking any agreements.
 */
import {
  Eve,
  Ferdie,
  registerProviderViaApi,
  deregisterProviderViaApi,
  disconnectApi,
} from "@web3-storage/test-helpers";

export default async function globalSetup() {
  await registerProviderViaApi(Eve, { multiaddr: "/ip4/127.0.0.1/tcp/3334" });
  await deregisterProviderViaApi(Ferdie);
  disconnectApi();
}
