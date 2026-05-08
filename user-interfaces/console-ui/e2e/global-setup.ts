/**
 * Playwright globalSetup — runs once before any spec.
 *
 * Cleans the provider registry to a single-Alice state so the runtime's
 * deterministic-but-hash-ordered `available_primary_providers[0]` always
 * picks Alice (the auto-coordinator). Console-ui's createBucket flow
 * doesn't strictly require provider acceptance to pass (no test waits on
 * primary_providers), but a clean registry avoids confusing UI rendering
 * when a stale provider account shows up in dropdowns.
 *
 * Per-spec beforeAlls used to call `registerProviderViaApi(Alice)` each
 * time; now that the global setup handles registration once, those calls
 * are redundant.
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
