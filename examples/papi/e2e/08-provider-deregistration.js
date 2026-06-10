/**
 * E2E Workflow 08 — Provider Deregistration
 *
 * Accounts: //Charlie (provider), //Dave (client)
 *
 * Note: DeregisterAnnouncementPeriod = 48 hours — too long for E2E.
 * We test announce + cancel paths. complete_deregister requires a
 * shorter period runtime.
 *
 * Usage: node e2e/08-provider-deregistration.js [chain_ws] [provider_url]
 */

import assert from "node:assert";
import {
  acceptAgreement,
  cancelDeregister,
  createBucket,
  deregisterProvider,
  ensureProviderRegistered,
  makeSigner,
  READ_OPTS,
  requestPrimaryAgreement,
  updateProviderSettings,
} from "@web3-storage/sdk";
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

  // Register Ferdie as a fresh provider with no prior agreements.
  // Charlie may have active agreements from earlier workflows (test 01 creates
  // one via createBucketWithStorage), so use Ferdie for deregister tests.
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
      await submitTxExpectFailure(tx, charlie.signer, "ProviderHasActiveAgreements", "8.4");
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

  tests.push({
    name: "8.8 Accept agreement after deregistration announcement (not accepting)",
    fn: async () => {
      // Since Charlie has active agreements and can't deregister, this test
      // verifies that a provider with accepting_primary=false is not matched.
      await updateProviderSettings(api, charlie, {
        min_duration: 10,
        max_duration: 100_000,
        price_per_byte: 1n,
        accepting_primary: false,
        replica_sync_price: undefined,
        accepting_extensions: true,
        max_capacity: 0n,
      });
      // Attempting to request an agreement with a non-accepting provider.
      const bucketId = await createBucket(api, dave);
      const tx = api.tx.StorageProvider.request_primary_agreement({
        bucket_id: bucketId,
        provider: charlie.address,
        max_bytes: 1_048_576n,
        duration: 50,
        max_payment: 1_048_576n * 50n * 10n,
      });
      await submitTxExpectFailure(tx, dave.signer, "ProviderNotAcceptingPrimary", "8.8");
      // Restore settings.
      await updateProviderSettings(api, charlie, {
        min_duration: 10,
        max_duration: 100_000,
        price_per_byte: 1n,
        accepting_primary: true,
        replica_sync_price: undefined,
        accepting_extensions: true,
        max_capacity: 0n,
      });
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
