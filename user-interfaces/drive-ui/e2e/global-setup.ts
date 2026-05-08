/**
 * Playwright globalSetup — runs once before any spec.
 *
 * Pre-registers Alice as a provider so per-spec beforeAll hits the warm
 * idempotent fast path. Without this, the first spec to run (drive-create.spec
 * alphabetically) shoulders the cold-chain finality + first-tx latency by
 * itself and can blow past its beforeAll timeout on a freshly-spawned chain.
 *
 * `registerProviderViaApi` is idempotent: if Alice is already registered (e.g.
 * because `just start-provider` already created her on the chain), the
 * register-tx is skipped; only update_provider_settings runs.
 */
import { Alice, registerProviderViaApi, disconnectApi } from "@web3-storage/test-helpers";

export default async function globalSetup() {
  await registerProviderViaApi(Alice);
  disconnectApi();
}
