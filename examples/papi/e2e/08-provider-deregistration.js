/**
 * E2E Workflow 08 — Provider Deregistration
 *
 * Tests the deregister_provider extrinsic.
 *
 * Accounts: //Alice (provider w/ node), //Charlie (bare provider), //Dave (client)
 *
 * Usage: node e2e/08-provider-deregistration.js [chain_ws] [provider_url]
 */

import assert from "node:assert";
import { deregisterProvider, endAgreement, registerProvider } from "../api.js";
import { ensureProviderRegistered, makeSigner, READ_OPTS } from "../common.js";
import {
  negotiateAndEstablish,
  runSuite,
  submitTxExpectFailure,
  setupChain,
} from "./helpers.js";

const CHAIN_WS = process.argv[2] || "ws://127.0.0.1:2222";
const PROVIDER_URL = process.argv[3] || "http://127.0.0.1:3333";

async function main() {
  const provider = makeSigner("//Alice");
  const charlie = makeSigner("//Charlie");
  const dave = makeSigner("//Dave");

  const { papi, api } = await setupChain(CHAIN_WS);

  // Alice runs the provider node — used for the active-agreement test.
  await ensureProviderRegistered(api, provider, PROVIDER_URL);

  // Charlie is a bare registration (no node) for the single-step deregister tests.
  await ensureProviderRegistered(api, charlie, PROVIDER_URL);

  const tests = [];

  // ── Success ───────────────────────────────────────────────────────────────

  tests.push({
    name: "8.1 Deregister succeeds with committed_bytes == 0",
    fn: async () => {
      const info = await api.query.StorageProvider.Providers.getValue(charlie.address, READ_OPTS);
      assert.strictEqual(info.committed_bytes, 0n, "Charlie should have no active agreements");

      const result = await deregisterProvider(api, charlie);
      const events = api.event.StorageProvider.ProviderDeregistered.filter(result.events);
      assert.strictEqual(events.length, 1, "Expected ProviderDeregistered event");
      assert.ok(events[0].stake_returned > 0n, "stake_returned should be non-zero");

      // Record is fully removed — not flagged.
      const after = await api.query.StorageProvider.Providers.getValue(charlie.address, READ_OPTS);
      assert.strictEqual(after, undefined, "Provider record must be absent after deregistration");
    },
  });

  tests.push({
    name: "8.2 Re-registration works after deregister",
    fn: async () => {
      // Proves the record was fully removed (not merely flagged).
      await registerProvider(api, charlie, PROVIDER_URL);
      const stored = await api.query.StorageProvider.Providers.getValue(charlie.address, READ_OPTS);
      assert.ok(stored, "Charlie should be re-registered");
    },
  });

  // ── Full lifecycle ────────────────────────────────────────────────────────

  tests.push({
    name: "8.3 Full deregister lifecycle (establish → end → deregister → re-register)",
    fn: async () => {
      // Step 1: dave opens an agreement against Alice.
      const { bucketId } = await negotiateAndEstablish(api, PROVIDER_URL, dave, provider, {
        maxBytes: 1_048_576n,
        duration: 100,
      });
      const infoBefore = await api.query.StorageProvider.Providers.getValue(provider.address, READ_OPTS);
      assert.ok(infoBefore.committed_bytes > 0n, "Alice should have committed_bytes > 0");

      // Step 2: deregister is blocked while committed.
      const txBlocked = api.tx.StorageProvider.deregister_provider();
      await submitTxExpectFailure(txBlocked, provider.signer, "ProviderHasActiveAgreements", "8.3-gate");

      // Step 3: dave (bucket owner/admin) early-terminates via Pay.
      await endAgreement(api, dave, provider, bucketId, "Pay");
      const infoAfterEnd = await api.query.StorageProvider.Providers.getValue(provider.address, READ_OPTS);
      assert.strictEqual(infoAfterEnd.committed_bytes, 0n, "committed_bytes should be 0 after end");

      // Step 4: deregister now succeeds.
      const result = await deregisterProvider(api, provider);
      const events = api.event.StorageProvider.ProviderDeregistered.filter(result.events);
      assert.strictEqual(events.length, 1, "Expected ProviderDeregistered event");
      const afterDeregister = await api.query.StorageProvider.Providers.getValue(provider.address, READ_OPTS);
      assert.strictEqual(afterDeregister, undefined, "Alice provider record must be absent after deregistration");

      // Step 5: Alice re-registers successfully.
      await registerProvider(api, provider, PROVIDER_URL);
      const reregistered = await api.query.StorageProvider.Providers.getValue(provider.address, READ_OPTS);
      assert.ok(reregistered, "Alice should be re-registered");
    },
  });

  tests.push({
    name: "8.4 Deregister fails for non-provider",
    fn: async () => {
      const tx = api.tx.StorageProvider.deregister_provider();
      await submitTxExpectFailure(tx, dave.signer, "ProviderNotFound", "8.4");
    },
  });

  await runSuite("08 — Provider Deregistration", tests, { api, papi });
  papi.destroy();
}

main().catch((err) => {
  console.error(err);
  process.exitCode = 1;
}).finally(() => {
  process.exit(process.exitCode || 0);
});
