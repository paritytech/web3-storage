/**
 * E2E Workflow 08 — Provider Deregistration
 *
 * Accounts: //Charlie (provider with active agreements), //Dave (client),
 *           //Ferdie (fresh provider, no agreements).
 *
 * Usage: node e2e/08-provider-deregistration.js [chain_ws] [provider_url]
 */

import assert from "node:assert";
import {
  acceptAgreement,
  createBucket,
  deregisterProvider,
  requestPrimaryAgreement,
} from "../api.js";
import {
  ensureProviderRegistered,
  makeSigner,
  READ_OPTS,
  sameAddress,
} from "../common.js";
import { runSuite, submitTxExpectFailure, setupChain } from "./helpers.js";

const CHAIN_WS = process.argv[2] || "ws://127.0.0.1:2222";
const PROVIDER_URL = process.argv[3] || "http://127.0.0.1:3333";


async function main() {
  const charlie = makeSigner("//Charlie");
  const dave = makeSigner("//Dave");
  const ferdie = makeSigner("//Ferdie");

  const { papi, api } = await setupChain(CHAIN_WS);

  // Ensure Charlie is registered (used for 8.4+ tests with active agreements).
  await ensureProviderRegistered(api, charlie, PROVIDER_URL);

  // Ferdie is a fresh provider with no prior agreements — used for the
  // success path where committed_bytes must be 0.
  await ensureProviderRegistered(api, ferdie, PROVIDER_URL);

  const tests = [];

  // ── Success ───────────────────────────────────────────────────────────────

  tests.push({
    name: "8.1 Deregister succeeds (no active agreements)",
    fn: async () => {
      const info = await api.query.StorageProvider.Providers.getValue(ferdie.address, READ_OPTS);
      assert.strictEqual(info.committed_bytes, 0n, "Ferdie should have no active agreements");

      const result = await deregisterProvider(api, ferdie);
      const events = api.event.StorageProvider.ProviderDeregistered.filter(result.events);
      assert.strictEqual(events.length, 1, "Expected ProviderDeregistered event");
      assert.ok(
        sameAddress(events[0].provider, ferdie.address),
        "Event provider should be Ferdie"
      );
      assert.strictEqual(
        events[0].stake_returned,
        info.stake,
        "stake_returned should equal Ferdie's registered stake"
      );

      const after = await api.query.StorageProvider.Providers.getValue(ferdie.address, READ_OPTS);
      assert.strictEqual(after, undefined, "Providers record should be removed after deregister");
    },
  });

  tests.push({
    name: "8.2 Re-registration works after deregister",
    fn: async () => {
      // The slot must be fully freed (not just flagged), so a fresh register works.
      await ensureProviderRegistered(api, ferdie, PROVIDER_URL);
      const stored = await api.query.StorageProvider.Providers.getValue(ferdie.address, READ_OPTS);
      assert.ok(stored, "Provider should exist after re-registration");
      assert.strictEqual(stored.settings.accepting_primary, true, "Should accept agreements after re-registration");
    },
  });

  // ── Failure ───────────────────────────────────────────────────────────────

  tests.push({
    name: "8.3 Deregister with active agreements",
    fn: async () => {
      // Create an agreement so Charlie has committed_bytes > 0.
      const maxBytes = 1_048_576n;
      const duration = 100;
      const bucketId = await createBucket(api, dave);
      await requestPrimaryAgreement(api, dave, charlie, bucketId, {
        max_bytes: maxBytes,
        duration,
        max_payment: maxBytes * BigInt(duration) * 10n,
      });
      // Manually accept — Charlie has no provider node running.
      await acceptAgreement(api, charlie, bucketId);
      const tx = api.tx.StorageProvider.deregister_provider();
      await submitTxExpectFailure(tx, charlie.signer, "ProviderHasActiveAgreements", "8.3");
    },
  });

  tests.push({
    name: "8.4 Non-provider deregisters",
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
