// SPDX-License-Identifier: Apache-2.0

/**
 * E2E Workflow 11 — Reconciler Resilience
 *
 * Accounts: //Alice (provider w/ node), //Dave (client)
 *
 * The provider node reads its own on-chain registration (settings + replay
 * window) in a background reconciler rather than once at startup. These tests
 * prove that, end to end:
 *
 *   11.1 `/info` exposes a readiness object and reports the running provider
 *        as signing-configured, registered, and nonce-counter-ready.
 *   11.2 A change to the provider's on-chain settings is picked up *without*
 *        restarting the node — a quote reflects the new listed price after a
 *        reconcile tick.
 *
 * 11.2 mutates Alice's listed price and restores it in a `finally`, so the
 * shared provider is left exactly as it was found. The node is run with a
 * short `--reconcile-interval-secs` in CI; the poll budget below also covers
 * the 30s default for local runs.
 *
 * Usage: node e2e/11-reconciler-resilience.js [chain_ws] [provider_url]
 */

import assert from "node:assert";
import {
  ensureProviderRegistered,
  makeSigner,
  READ_OPTS,
  updateProviderSettings,
  type ChainSigner,
  type ParachainApi,
} from "@web3-storage/sdk";
import { negotiateSigned, runSuite, setupChain } from "./helpers.js";

const CHAIN_WS = process.argv[2] || "ws://127.0.0.1:2222";
const PROVIDER_URL = process.argv[3] || "http://127.0.0.1:3333";

// Poll budget for a reconcile tick to land. CI runs the provider with
// `--reconcile-interval-secs 1`, so this resolves in ~1s; the 60s ceiling
// covers the 30s default a developer gets from a plain `just start-provider`.
const POLL_TIMEOUT_MS = 60_000;
const POLL_INTERVAL_MS = 1_000;

/**
 * Poll `predicate` until it resolves truthy or the budget elapses. A predicate
 * that throws (e.g. a quote rejected as underpriced before the reconciler has
 * caught up) is treated as "not yet" and retried.
 */
async function waitFor(
  predicate: () => Promise<boolean>,
  label: string,
): Promise<void> {
  const deadline = Date.now() + POLL_TIMEOUT_MS;
  let lastErr: unknown;
  while (Date.now() < deadline) {
    try {
      if (await predicate()) return;
    } catch (err) {
      lastErr = err;
    }
    await new Promise((r) => setTimeout(r, POLL_INTERVAL_MS));
  }
  throw new Error(
    `${label}: condition not met within ${POLL_TIMEOUT_MS}ms` +
      (lastErr ? ` (last error: ${(lastErr as Error).message})` : ""),
  );
}

/** Quote shape that fits Alice's duration/capacity bounds. */
const QUOTE = { maxBytes: 1024n, duration: 50 };

/** Negotiate at `pricePerByte` and return the price the provider pinned. */
async function quotedPrice(
  api: ParachainApi,
  owner: ChainSigner,
  provider: ChainSigner,
  pricePerByte: bigint,
): Promise<bigint> {
  const signed = await negotiateSigned(api, PROVIDER_URL, owner, provider, {
    ...QUOTE,
    pricePerByte,
  });
  return BigInt(signed.terms.price_per_byte);
}

async function main() {
  const provider = makeSigner("//Alice"); // the account the node runs as
  const owner = makeSigner("//Dave");

  const { papi, api } = await setupChain(CHAIN_WS);
  await ensureProviderRegistered(api, provider, PROVIDER_URL);

  const tests: Array<{ name: string; fn: () => Promise<void> }> = [];

  tests.push({
    name: "11.1 /info reports the running provider as ready",
    fn: async () => {
      const res = await fetch(`${PROVIDER_URL}/info`);
      assert.ok(res.ok, `/info returned ${res.status}`);
      const info = await res.json();
      assert.ok(info.readiness, "/info must expose a readiness object");
      assert.strictEqual(
        info.readiness.signing_configured,
        true,
        "node has a --keyfile, so signing must be configured",
      );
      assert.strictEqual(
        info.readiness.provider_info_loaded,
        true,
        "Alice is registered, so the reconciler must have loaded provider_info",
      );
      assert.strictEqual(
        info.readiness.nonce_counter_ready,
        true,
        "the nonce counter must be bootstrapped from the replay window",
      );
    },
  });

  tests.push({
    name: "11.2 Reconciler picks up settings changes without a restart",
    fn: async () => {
      const stored = (await api.query.StorageProvider.Providers.getValue(
        provider.address,
        READ_OPTS,
      ))!;
      const base = {
        min_duration: stored.settings.min_duration,
        max_duration: stored.settings.max_duration,
        price_per_byte: stored.settings.price_per_byte,
        accepting_primary: stored.settings.accepting_primary,
        replica_sync_price: stored.settings.replica_sync_price ?? undefined,
        accepting_extensions: stored.settings.accepting_extensions,
        max_capacity: stored.settings.max_capacity,
      };
      const originalPrice = BigInt(base.price_per_byte);
      const bumpedPrice = originalPrice + 1n;

      try {
        // Raise the listed price on chain. The node read the old price at
        // startup; only the reconciler can update its in-memory view.
        await updateProviderSettings(api, provider, {
          ...base,
          price_per_byte: bumpedPrice,
        });

        // Before the refresh a quote at `bumpedPrice` is pinned to the old,
        // lower listed price; after it, to the new one. Poll for the new price.
        await waitFor(
          async () =>
            (await quotedPrice(api, owner, provider, bumpedPrice)) === bumpedPrice,
          "reconciler did not pick up the bumped listed price",
        );
      } finally {
        // Restore the original price and confirm the node converges back, so we
        // leave the shared provider exactly as we found it for later runs.
        await updateProviderSettings(api, provider, base);
        await waitFor(
          async () =>
            (await quotedPrice(api, owner, provider, originalPrice)) === originalPrice,
          "reconciler did not converge back to the original listed price",
        );
      }
    },
  });

  await runSuite("11 — Reconciler Resilience", tests, { api, papi });
  papi.destroy();
}

main()
  .catch((err) => {
    console.error(err);
    process.exitCode = 1;
  })
  .finally(() => {
    process.exit(process.exitCode || 0);
  });
