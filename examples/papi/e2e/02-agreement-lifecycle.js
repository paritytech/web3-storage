/**
 * E2E Workflow 02 — Agreement Lifecycle
 *
 * Accounts: //Alice (provider), //Bob (client), //Charlie (non-party)
 *
 * Agreements are opened by redeeming provider-signed terms via
 * `establish_storage_agreement` — the bucket is created atomically and there
 * is no separate request/accept/reject/withdraw step. Tests cover establish,
 * end (Pay/Burn), and the on-chain/off-chain rejections that replace the old
 * request-flow failures.
 *
 * Usage: node e2e/02-agreement-lifecycle.js [chain_ws] [provider_url]
 */

import assert from "node:assert";
import { Enum } from "@polkadot-api/substrate-bindings";
import { buildSignedTermsArgs, endAgreement, negotiateTerms } from "../api.js";
import {
  ensureProviderRegistered,
  makeSigner,
  READ_OPTS,
  sameAddress,
  waitForBlock,
} from "../common.js";
import {
  getFree,
  negotiateAndEstablish,
  negotiateSigned,
  runSuite,
  setupChain,
  submitTxExpectFailure,
} from "./helpers.js";

const CHAIN_WS = process.argv[2] || "ws://127.0.0.1:2222";
const PROVIDER_URL = process.argv[3] || "http://127.0.0.1:3333";


async function main() {
  const provider = makeSigner("//Alice");
  const client = makeSigner("//Bob");
  const charlie = makeSigner("//Charlie");

  const { papi, api } = await setupChain(CHAIN_WS);
  await ensureProviderRegistered(api, provider, PROVIDER_URL);

  const maxBytes = 1_048_576n; // 1 MiB
  const duration = 10;

  const tests = [];

  // ── Success ───────────────────────────────────────────────────────────────

  tests.push({
    name: "2.1 Establish storage agreement",
    fn: async () => {
      const { bucketId } = await negotiateAndEstablish(
        api,
        PROVIDER_URL,
        client,
        provider,
        { maxBytes, duration }
      );
      // Bucket created with the client as admin and the provider as primary.
      const bucket = await api.query.StorageProvider.Buckets.getValue(bucketId, READ_OPTS);
      assert.ok(
        bucket.primary_providers.some((p) => sameAddress(p, provider.address)),
        "Provider should be in primary_providers"
      );
      const agreement = await api.query.StorageProvider.StorageAgreements.getValue(
        bucketId,
        provider.address,
        READ_OPTS
      );
      assert.ok(agreement, "Agreement should exist immediately after establish");
      assert.strictEqual(agreement.max_bytes, maxBytes, "max_bytes should match the signed terms");
    },
  });

  tests.push({
    name: "2.2 End agreement (Pay)",
    fn: async () => {
      const { bucketId } = await negotiateAndEstablish(
        api,
        PROVIDER_URL,
        client,
        provider,
        { maxBytes, duration }
      );
      const agreement = await api.query.StorageProvider.StorageAgreements.getValue(
        bucketId,
        provider.address,
        READ_OPTS
      );
      const expiresAt = Number(agreement.expires_at);
      console.log("    Waiting for expiry at block %d...", expiresAt);
      await waitForBlock(papi, expiresAt);
      const provBefore = await getFree(api, provider);
      await endAgreement(api, client, provider, bucketId, "Pay");
      const provAfter = await getFree(api, provider);
      assert.ok(provAfter > provBefore, "Provider balance should increase after Pay");
    },
  });

  tests.push({
    name: "2.3 End agreement (Burn)",
    fn: async () => {
      // Use larger max_bytes so payment_locked (= price_per_byte × max_bytes × duration)
      // exceeds the existential deposit. With price_per_byte=1 and duration=10:
      //   1 × 1_073_741_824 × 10 = 10,737,418,240 > ED (1,000,000,000)
      // Otherwise the treasury transfer fails with Token::BelowMinimum when the
      // treasury account doesn't exist yet.
      const burnMaxBytes = 1_073_741_824n; // 1 GiB
      const { bucketId } = await negotiateAndEstablish(
        api,
        PROVIDER_URL,
        client,
        provider,
        { maxBytes: burnMaxBytes, duration }
      );
      const agreement = await api.query.StorageProvider.StorageAgreements.getValue(
        bucketId,
        provider.address,
        READ_OPTS
      );
      console.log("    Waiting for expiry at block %d...", Number(agreement.expires_at));
      await waitForBlock(papi, Number(agreement.expires_at));
      const result = await endAgreement(api, client, provider, bucketId, "Burn", { burn_percent: 100 });
      const events = api.event.StorageProvider.AgreementEnded.filter(result.events);
      assert.strictEqual(events.length, 1, "Expected AgreementEnded event");
    },
  });

  // ── Failure ───────────────────────────────────────────────────────────────

  tests.push({
    name: "2.4 Non-owner ends agreement",
    fn: async () => {
      const { bucketId } = await negotiateAndEstablish(
        api,
        PROVIDER_URL,
        client,
        provider,
        { maxBytes, duration: 50 }
      );
      // Charlie is not bucket admin — should fail.
      const tx = api.tx.StorageProvider.end_agreement({
        bucket_id: bucketId,
        provider: provider.address,
        action: Enum("Pay"),
      });
      await submitTxExpectFailure(tx, charlie.signer, "NotBucketAdmin", "2.4");
    },
  });

  tests.push({
    name: "2.5 Owner mismatch on redeem",
    fn: async () => {
      // Terms are signed for Bob; Charlie must not be able to redeem them.
      const signed = await negotiateSigned(api, PROVIDER_URL, client, provider, {
        maxBytes,
        duration,
      });
      const tx = api.tx.StorageProvider.establish_storage_agreement(
        buildSignedTermsArgs(provider, signed)
      );
      await submitTxExpectFailure(tx, charlie.signer, "TermsOwnerMismatch", "2.5");
    },
  });

  tests.push({
    name: "2.6 Nonce replay rejected",
    fn: async () => {
      // Redeem a quote once, then replay the exact same signed bundle.
      const signed = await negotiateSigned(api, PROVIDER_URL, client, provider, {
        maxBytes,
        duration,
      });
      const args = buildSignedTermsArgs(provider, signed);
      await api.tx.StorageProvider.establish_storage_agreement(args)
        .signAndSubmit(client.signer);
      const replay = api.tx.StorageProvider.establish_storage_agreement(args);
      await submitTxExpectFailure(replay, client.signer, "NonceAlreadyUsed", "2.6");
    },
  });

  tests.push({
    name: "2.7 Provider refuses duration too short",
    fn: async () => {
      // The provider node signs only within its [min_duration, max_duration]
      // window, so an out-of-range quote is rejected off-chain (HTTP 422).
      await assertNegotiateRejects(
        () => negotiateSigned(api, PROVIDER_URL, client, provider, { maxBytes, duration: 1 }),
        "duration_out_of_bounds",
        "2.7"
      );
    },
  });

  tests.push({
    name: "2.8 Provider refuses duration too long",
    fn: async () => {
      await assertNegotiateRejects(
        () => negotiateSigned(api, PROVIDER_URL, client, provider, {
          maxBytes,
          duration: 999_999_999,
        }),
        "duration_out_of_bounds",
        "2.8"
      );
    },
  });

  tests.push({
    name: "2.9 Provider refuses price below listed",
    fn: async () => {
      // The node refuses to sign terms priced below its listed
      // price_per_byte (Alice lists 1), since the chain treats its
      // signature as consent to those terms.
      await assertNegotiateRejects(
        () =>
          negotiateTerms(PROVIDER_URL, {
            owner: client.address,
            max_bytes: maxBytes,
            duration,
            pricePerByte: 0n,
            replica_params: null,
            bucket_id: null,
          }),
        "price_below_listed",
        "2.9"
      );
    },
  });

  await runSuite("02 — Agreement Lifecycle", tests, { api, papi });
  papi.destroy();
}

/**
 * Assert that a `/negotiate` call rejects with the expected error code. The
 * `negotiateTerms` helper throws `"/negotiate failed: <status> <body>"` on a
 * non-2xx response, and the body carries the machine-readable error string.
 */
async function assertNegotiateRejects(fn, expectedError, label) {
  try {
    await fn();
  } catch (err) {
    if (err.message && err.message.includes(expectedError)) return;
    throw new Error(
      `${label}: expected /negotiate to reject with "${expectedError}", got "${err.message}"`
    );
  }
  throw new Error(`${label}: expected /negotiate to reject with "${expectedError}", but it succeeded`);
}

main().catch((err) => {
  console.error(err);
  process.exitCode = 1;
}).finally(() => {
  process.exit(process.exitCode || 0);
});
