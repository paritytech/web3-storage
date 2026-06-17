// SPDX-License-Identifier: Apache-2.0

/**
 * E2E Workflow 08 — Provider Deregistration
 *
 * Accounts: //Alice (provider w/ node), //Ferdie (bare provider), //Dave (client)
 *
 * Note: DeregisterAnnouncementPeriod = 48 hours — too long for E2E.
 * We test announce + cancel paths. complete_deregister requires a
 * shorter period runtime.
 *
 * Agreements are now opened by redeeming provider-signed terms, so the
 * "active agreements block deregister" test uses Alice (the only provider
 * with a node that can sign valid terms). Ferdie is a bare registration used
 * for the announce/cancel paths, which need no agreement and no node.
 *
 * Usage: node e2e/08-provider-deregistration.js [chain_ws] [provider_url]
 */

import assert from "node:assert";
import { cancelDeregister, deregisterProvider, updateProviderSettings } from "../api.js";
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
  const dave = makeSigner("//Dave");
  const ferdie = makeSigner("//Ferdie");

  const { papi, api } = await setupChain(CHAIN_WS);

  // Alice runs the provider node — used for the active-agreement test, since
  // only her key produces signatures the chain accepts.
  await ensureProviderRegistered(api, provider, PROVIDER_URL);

  // Ferdie is a bare provider registration (no node) for announce/cancel.
  await ensureProviderRegistered(api, ferdie, PROVIDER_URL);

  const tests = [];

  // ── Success ───────────────────────────────────────────────────────────────

  tests.push({
    name: "8.1 Announce deregistration (no active agreements)",
    fn: async () => {
      const info = await api.query.StorageProvider.Providers.getValue(ferdie.address, READ_OPTS);
      assert.strictEqual(info.committed_bytes, 0n, "Ferdie should have no active agreements");
      const result = await deregisterProvider(api, ferdie);
      const events = api.event.StorageProvider.DeregisterAnnounced.filter(result.events);
      assert.strictEqual(events.length, 1, "Expected DeregisterAnnounced event");
      const after = await api.query.StorageProvider.Providers.getValue(ferdie.address, READ_OPTS);
      assert.strictEqual(
        after.settings.accepting_primary,
        false,
        "accepting_primary should be false after deregister announcement"
      );
    },
  });

  tests.push({
    name: "8.2 Cancel deregistration",
    fn: async () => {
      const result = await cancelDeregister(api, ferdie);
      const events = api.event.StorageProvider.DeregisterCancelled.filter(result.events);
      assert.strictEqual(events.length, 1, "Expected DeregisterCancelled event");
      const after = await api.query.StorageProvider.Providers.getValue(ferdie.address, READ_OPTS);
      assert.ok(after, "Provider should still exist after cancel");
    },
  });

  tests.push({
    name: "8.3 Re-registration works after cancel",
    fn: async () => {
      await updateProviderSettings(api, ferdie, {
        min_duration: 10,
        max_duration: 100_000,
        price_per_byte: 1n,
        accepting_primary: true,
        replica_sync_price: undefined,
        accepting_extensions: true,
        max_capacity: 0n,
      });
      const stored = await api.query.StorageProvider.Providers.getValue(ferdie.address, READ_OPTS);
      assert.strictEqual(stored.settings.accepting_primary, true, "Should accept agreements again");
    },
  });

  // ── Failure ───────────────────────────────────────────────────────────────

  tests.push({
    name: "8.4 Deregister with active agreements",
    fn: async () => {
      // Open an agreement against Alice so she has committed_bytes > 0, then
      // confirm she can't announce deregistration while it's active.
      await negotiateAndEstablish(api, PROVIDER_URL, dave, provider, {
        maxBytes: 1_048_576n,
        duration: 100,
      });
      const info = await api.query.StorageProvider.Providers.getValue(provider.address, READ_OPTS);
      assert.ok(info.committed_bytes > 0n, "Alice should have an active agreement");
      const tx = api.tx.StorageProvider.deregister_provider();
      await submitTxExpectFailure(tx, provider.signer, "ProviderHasActiveAgreements", "8.4");
    },
  });

  tests.push({
    name: "8.5 Complete before period elapsed",
    fn: async () => {
      // Ferdie hasn't announced deregistration (8.2 cancelled it).
      // Test that complete_deregister fails when not announced.
      const tx = api.tx.StorageProvider.complete_deregister();
      await submitTxExpectFailure(tx, ferdie.signer, "DeregisterNotAnnounced", "8.5");
    },
  });

  tests.push({
    name: "8.6 Cancel without announcement",
    fn: async () => {
      // Ferdie hasn't announced (8.2 cancelled it, and no new announcement since).
      const tx = api.tx.StorageProvider.cancel_deregister();
      await submitTxExpectFailure(tx, ferdie.signer, "DeregisterNotAnnounced", "8.6");
    },
  });

  tests.push({
    name: "8.7 Non-provider deregisters",
    fn: async () => {
      const tx = api.tx.StorageProvider.deregister_provider();
      await submitTxExpectFailure(tx, dave.signer, "ProviderNotFound", "8.7");
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
