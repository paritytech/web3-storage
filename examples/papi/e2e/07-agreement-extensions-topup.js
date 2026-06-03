/**
 * E2E Workflow 07 — Agreement Extensions & Top-up
 *
 * Accounts: //Alice (provider), //Bob (client), //Charlie (non-party)
 *
 * Tests: extend duration, top up bytes, block extensions, failure cases.
 *
 * Usage: node e2e/07-agreement-extensions-topup.js [chain_ws] [provider_url]
 */

import assert from "node:assert";
import {
  createBucket,
  extendAgreement,
  requestPrimaryAgreement,
  setExtensionsBlocked,
  topUpAgreement,
  updateProviderSettings,
} from "../api.js";
import {
  ensureProviderRegistered,
  makeSigner,
  waitForAgreementAcceptance,
  waitForBlock,
} from "../common.js";
import { runSuite, submitTxExpectFailure, setupChain, getFree } from "./helpers.js";

const CHAIN_WS = process.argv[2] || "ws://127.0.0.1:2222";
const PROVIDER_URL = process.argv[3] || "http://127.0.0.1:3333";

async function main() {
  const provider = makeSigner("//Alice");
  const client = makeSigner("//Bob");
  const charlie = makeSigner("//Charlie");

  const { papi, api } = await setupChain(CHAIN_WS);
  await ensureProviderRegistered(api, provider, PROVIDER_URL, {
    pricePerByte: 1n,
    maxDuration: 100_000,
  });

  // Create a bucket with a longer-lived agreement for extension tests.
  const maxBytes = 1_048_576n;
  const duration = 100;
  const maxPayment = maxBytes * BigInt(duration) * 10n;

  let bucketId;

  const tests = [];

  tests.push({
    name: "7.0 Setup bucket + agreement",
    fn: async () => {
      bucketId = await createBucket(api, client);
      await requestPrimaryAgreement(api, client, provider, bucketId, {
        max_bytes: maxBytes,
        duration,
        max_payment: maxPayment,
      });
      await waitForAgreementAcceptance(api, provider.address, bucketId);
      const agreement = await api.query.StorageProvider.StorageAgreements.getValue(
        bucketId,
        provider.address
      );
      assert.ok(agreement, "Agreement should exist");
    },
  });

  // ── Success ───────────────────────────────────────────────────────────────

  tests.push({
    name: "7.1 Extend agreement duration",
    fn: async () => {
      const before = await api.query.StorageProvider.StorageAgreements.getValue(
        bucketId,
        provider.address
      );
      const extDuration = 25;
      const result = await extendAgreement(api, client, bucketId, provider, {
        duration: extDuration,
        max_payment: maxBytes * BigInt(extDuration) * 10n,
      });
      const events = api.event.StorageProvider.AgreementExtended.filter(result.events);
      assert.strictEqual(events.length, 1, "Expected AgreementExtended event");
      const after = await api.query.StorageProvider.StorageAgreements.getValue(
        bucketId,
        provider.address
      );
      assert.ok(after.expires_at > before.expires_at, "expires_at should increase");
    },
  });

  tests.push({
    name: "7.2 Top up bytes",
    fn: async () => {
      const before = await api.query.StorageProvider.StorageAgreements.getValue(
        bucketId,
        provider.address
      );
      const extraBytes = 524_288n; // 512 KB
      const result = await topUpAgreement(api, client, bucketId, provider, {
        additional_bytes: extraBytes,
        max_payment: extraBytes * BigInt(duration) * 10n,
      });
      const events = api.event.StorageProvider.AgreementToppedUp.filter(result.events);
      assert.strictEqual(events.length, 1, "Expected AgreementToppedUp event");
      const after = await api.query.StorageProvider.StorageAgreements.getValue(
        bucketId,
        provider.address
      );
      assert.ok(after.max_bytes > before.max_bytes, "max_bytes should increase");
    },
  });

  // ── Failure ───────────────────────────────────────────────────────────────

  tests.push({
    name: "7.3 Block extensions per-agreement",
    fn: async () => {
      await setExtensionsBlocked(api, client, bucketId, provider, true);
      const tx = api.tx.StorageProvider.extend_agreement({
        bucket_id: bucketId,
        provider: provider.address,
        duration: 10,
        max_payment: maxBytes * 10n * 10n,
      });
      await submitTxExpectFailure(tx, client.signer, "AgreementExtensionsBlocked", "7.3");
      // Unblock for further tests.
      await setExtensionsBlocked(api, client, bucketId, provider, false);
    },
  });

  tests.push({
    name: "7.4 Provider disables extensions globally",
    fn: async () => {
      await updateProviderSettings(api, provider, {
        min_duration: 10,
        max_duration: 100_000,
        price_per_byte: 1n,
        accepting_primary: true,
        replica_sync_price: undefined,
        accepting_extensions: false,
        max_capacity: 0n,
      });
      const tx = api.tx.StorageProvider.extend_agreement({
        bucket_id: bucketId,
        provider: provider.address,
        duration: 10,
        max_payment: maxBytes * 10n * 10n,
      });
      await submitTxExpectFailure(tx, client.signer, "ProviderNotAcceptingExtensions", "7.4");
      // Restore extensions.
      await updateProviderSettings(api, provider, {
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

  tests.push({
    name: "7.5 Non-owner extends",
    fn: async () => {
      const tx = api.tx.StorageProvider.extend_agreement({
        bucket_id: bucketId,
        provider: provider.address,
        duration: 10,
        max_payment: maxBytes * 10n * 10n,
      });
      await submitTxExpectFailure(tx, charlie.signer, "NotBucketAdmin", "7.5");
    },
  });

  tests.push({
    name: "7.6 Payment too low on extend",
    fn: async () => {
      const tx = api.tx.StorageProvider.extend_agreement({
        bucket_id: bucketId,
        provider: provider.address,
        duration: 10,
        max_payment: 1n, // Way too low
      });
      await submitTxExpectFailure(tx, client.signer, "PaymentExceedsMax", "7.6");
    },
  });

  await runSuite("07 — Agreement Extensions & Top-up", tests, { api, papi });
  papi.destroy();
}

main();
