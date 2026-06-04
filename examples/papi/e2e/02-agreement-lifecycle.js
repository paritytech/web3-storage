/**
 * E2E Workflow 02 — Agreement Lifecycle
 *
 * Accounts: //Alice (provider), //Bob (client), //Charlie (non-party)
 *
 * Tests: request, accept, reject, withdraw, end (Pay/Burn).
 *
 * Usage: node e2e/02-agreement-lifecycle.js [chain_ws] [provider_url]
 */

import assert from "node:assert";
import {
  createBucket,
  endAgreement,
  rejectAgreement,
  requestPrimaryAgreement,
  withdrawAgreementRequest,
} from "../api.js";
import {
  ensureProviderRegistered,
  makeSigner,
  sameAddress,
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
  await ensureProviderRegistered(api, provider, PROVIDER_URL);

  // Shared state across tests
  let mainBucketId;
  let rejectBucketId;
  let withdrawBucketId;
  let burnBucketId;

  const maxBytes = 1_048_576n; // 1 MiB
  const duration = 10;
  const maxPayment = maxBytes * BigInt(duration) * 10n;

  const tests = [];

  // ── Success ───────────────────────────────────────────────────────────────

  tests.push({
    name: "2.1 Request primary agreement",
    fn: async () => {
      mainBucketId = await createBucket(api, client);
      const result = await requestPrimaryAgreement(api, client, provider, mainBucketId, {
        max_bytes: maxBytes,
        duration,
        max_payment: maxPayment,
      });
      const events = api.event.StorageProvider.AgreementRequested.filter(result.events);
      assert.strictEqual(events.length, 1, "Expected AgreementRequested event");
    },
  });

  tests.push({
    name: "2.2 Accept agreement (auto-accept)",
    fn: async () => {
      await waitForAgreementAcceptance(api, provider.address, mainBucketId);
      const agreement = await api.query.StorageProvider.StorageAgreements.getValue(
        mainBucketId,
        provider.address
      );
      assert.ok(agreement, "Agreement should exist after acceptance");
      const bucket = await api.query.StorageProvider.Buckets.getValue(mainBucketId);
      assert.ok(
        bucket.primary_providers.some((p) => sameAddress(p, provider.address)),
        "Provider should be in primary_providers"
      );
    },
  });

  tests.push({
    name: "2.3 Reject agreement",
    fn: async () => {
      rejectBucketId = await createBucket(api, client);
      await requestPrimaryAgreement(api, client, provider, rejectBucketId, {
        max_bytes: maxBytes,
        duration,
        max_payment: maxPayment,
      });
      const balAfterRequest = await getFree(api, client);
      const result = await rejectAgreement(api, provider, rejectBucketId);
      const events = api.event.StorageProvider.AgreementRejected.filter(result.events);
      assert.strictEqual(events.length, 1, "Expected AgreementRejected event");
      // Locked payment is returned after reject. Balance should increase
      // compared to after the request (client still lost tx fees for
      // create_bucket + request, but the reserved payment is unreserved).
      const balAfterReject = await getFree(api, client);
      assert.ok(
        balAfterReject > balAfterRequest,
        `Balance should increase after reject: ${balAfterReject} > ${balAfterRequest}`
      );
      const req = await api.query.StorageProvider.AgreementRequests.getValue(
        rejectBucketId,
        provider.address
      );
      assert.strictEqual(req, undefined, "Request should be gone after reject");
    },
  });

  tests.push({
    name: "2.4 Withdraw agreement request",
    fn: async () => {
      withdrawBucketId = await createBucket(api, client);
      await requestPrimaryAgreement(api, client, provider, withdrawBucketId, {
        max_bytes: maxBytes,
        duration,
        max_payment: maxPayment,
      });
      const result = await withdrawAgreementRequest(api, client, withdrawBucketId, provider);
      const events = api.event.StorageProvider.AgreementRequestWithdrawn.filter(result.events);
      assert.strictEqual(events.length, 1, "Expected AgreementRequestWithdrawn event");
    },
  });

  tests.push({
    name: "2.5 End agreement (Pay)",
    fn: async () => {
      const agreement = await api.query.StorageProvider.StorageAgreements.getValue(
        mainBucketId,
        provider.address
      );
      const expiresAt = Number(agreement.expires_at);
      console.log("    Waiting for expiry at block %d...", expiresAt);
      await waitForBlock(papi, expiresAt);
      const provBefore = await getFree(api, provider);
      await endAgreement(api, client, provider, mainBucketId, "Pay");
      const provAfter = await getFree(api, provider);
      assert.ok(provAfter > provBefore, "Provider balance should increase after Pay");
    },
  });

  tests.push({
    name: "2.6 End agreement (Burn)",
    fn: async () => {
      burnBucketId = await createBucket(api, client);
      await requestPrimaryAgreement(api, client, provider, burnBucketId, {
        max_bytes: maxBytes,
        duration,
        max_payment: maxPayment,
      });
      await waitForAgreementAcceptance(api, provider.address, burnBucketId);
      const agreement = await api.query.StorageProvider.StorageAgreements.getValue(
        burnBucketId,
        provider.address
      );
      console.log("    Waiting for expiry at block %d...", Number(agreement.expires_at));
      await waitForBlock(papi, Number(agreement.expires_at));
      const result = await endAgreement(api, client, provider, burnBucketId, "Burn", { burn_percent: 100 });
      const events = api.event.StorageProvider.AgreementEnded.filter(result.events);
      assert.strictEqual(events.length, 1, "Expected AgreementEnded event");
    },
  });

  // ── Failure ───────────────────────────────────────────────────────────────

  tests.push({
    name: "2.7 Payment too low",
    fn: async () => {
      const bucketId = await createBucket(api, client);
      const tx = api.tx.StorageProvider.request_primary_agreement({
        bucket_id: bucketId,
        provider: provider.address,
        max_bytes: maxBytes,
        duration,
        max_payment: 1n, // Way too low
      });
      await submitTxExpectFailure(tx, client.signer, "PaymentExceedsMax", "2.7");
    },
  });

  tests.push({
    name: "2.8 Duration too short",
    fn: async () => {
      const bucketId = await createBucket(api, client);
      const tx = api.tx.StorageProvider.request_primary_agreement({
        bucket_id: bucketId,
        provider: provider.address,
        max_bytes: maxBytes,
        duration: 1, // Below min_duration
        max_payment: maxPayment,
      });
      await submitTxExpectFailure(tx, client.signer, "DurationTooShort", "2.8");
    },
  });

  tests.push({
    name: "2.9 Duration too long",
    fn: async () => {
      const bucketId = await createBucket(api, client);
      const tx = api.tx.StorageProvider.request_primary_agreement({
        bucket_id: bucketId,
        provider: provider.address,
        max_bytes: maxBytes,
        duration: 999_999_999,
        max_payment: maxBytes * 999_999_999n * 10n,
      });
      await submitTxExpectFailure(tx, client.signer, "DurationTooLong", "2.9");
    },
  });

  tests.push({
    name: "2.10 Duplicate agreement request",
    fn: async () => {
      const bucketId = await createBucket(api, client);
      await requestPrimaryAgreement(api, client, provider, bucketId, {
        max_bytes: maxBytes,
        duration,
        max_payment: maxPayment,
      });
      // Second request for the same provider+bucket should fail.
      const tx = api.tx.StorageProvider.request_primary_agreement({
        bucket_id: bucketId,
        provider: provider.address,
        max_bytes: maxBytes,
        duration,
        max_payment: maxPayment,
      });
      await submitTxExpectFailure(tx, client.signer, "AgreementRequestAlreadyExists", "2.10");
    },
  });

  tests.push({
    name: "2.11 Accept non-existent request",
    fn: async () => {
      const emptyBucket = await createBucket(api, client);
      const tx = api.tx.StorageProvider.accept_agreement({ bucket_id: emptyBucket });
      await submitTxExpectFailure(tx, provider.signer, "AgreementRequestNotFound", "2.11");
    },
  });

  tests.push({
    name: "2.12 Non-owner ends agreement",
    fn: async () => {
      // Create a fresh agreement for this test.
      const bucketId = await createBucket(api, client);
      await requestPrimaryAgreement(api, client, provider, bucketId, {
        max_bytes: maxBytes,
        duration: 50,
        max_payment: maxBytes * 50n * 10n,
      });
      await waitForAgreementAcceptance(api, provider.address, bucketId);
      // Charlie is not bucket admin — should fail.
      const tx = api.tx.StorageProvider.end_agreement({
        bucket_id: bucketId,
        provider: provider.address,
        action: (await import("@polkadot-api/substrate-bindings")).Enum("Pay"),
      });
      await submitTxExpectFailure(tx, charlie.signer, "NotBucketAdmin", "2.12");
    },
  });

  await runSuite("02 — Agreement Lifecycle", tests, { api, papi });
  papi.destroy();
}

main().catch((err) => {
  console.error(err);
  process.exitCode = 1;
}).finally(() => {
  process.exit(process.exitCode || 0);
});
