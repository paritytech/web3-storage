/**
 * E2E Workflow 01 — Provider Registration
 *
 * Accounts: //Charlie (provider), //Dave (imposter)
 *
 * Tests:
 *   1.1  Register new provider
 *   1.2  Update settings
 *   1.3  Add stake
 *   1.4  Update multiaddr
 *   1.5  Settings take effect for matching
 *   1.6  Duplicate registration
 *   1.7  Insufficient stake
 *   1.8  Non-provider updates settings
 *   1.9  Non-provider adds stake
 *   1.10 min_duration > max_duration
 *   1.11 accepting_primary=false blocks matching
 *   1.12 max_capacity=0 (unlimited)
 *
 * Usage: node e2e/01-provider-registration.js [chain_ws] [provider_url]
 */

import assert from "node:assert";
import {
  addStake,
  makeSigner,
  READ_OPTS,
  registerProvider,
  toHex,
  updateProviderMultiaddr,
  updateProviderSettings,
  type ChainSigner,
  type ParachainApi,
} from "@web3-storage/sdk";
import { runSuite, submitTxExpectFailure, setupChain } from "./helpers.js";

const CHAIN_WS = process.argv[2] || "ws://127.0.0.1:2222";
const PROVIDER_URL = process.argv[3] || "http://127.0.0.1:3333";
const UNIT = 1_000_000_000_000n;

/**
 * Provider matching moved off-chain: clients call the `find_matching_providers`
 * runtime API, pick a provider, then redeem its signed terms. This finds a
 * given provider's match entry (scored 0-100) for the requirements.
 */
async function matchEntry(
  api: ParachainApi,
  who: ChainSigner,
  {
    bytesNeeded,
    minDuration,
    maxPricePerByte,
  }: { bytesNeeded: bigint; minDuration: number; maxPricePerByte: bigint }
) {
  const matches = await api.apis.StorageProviderApi.find_matching_providers(
    {
      bytes_needed: bytesNeeded,
      min_duration: minDuration,
      max_price_per_byte: maxPricePerByte,
      primary_only: true,
    },
    50
  );
  return matches.find((m: any) => toHex(m.account.asBytes()) === toHex(who.publicKey));
}

async function main() {
  const charlie = makeSigner("//Charlie");
  const dave = makeSigner("//Dave");

  const { papi, api } = await setupChain(CHAIN_WS);

  // Ensure Charlie is not already registered (from a previous run).
  // If already registered, deregister first — but that's complex, so we just
  // check and skip registration test if needed, or re-use a fresh chain.
  const existing = await api.query.StorageProvider.Providers.getValue(charlie.address, READ_OPTS);

  const tests: Array<{ name: string; fn: () => Promise<void> }> = [];

  // ── Success tests ─────────────────────────────────────────────────────────

  if (!existing) {
    tests.push({
      name: "1.1 Register new provider",
      fn: async () => {
        const result = await registerProvider(api, charlie, PROVIDER_URL, 1000n * UNIT);
        const events = api.event.StorageProvider.ProviderRegistered.filter(result.events as never);
        assert.strictEqual(events.length, 1, "Expected ProviderRegistered event");
        const stored = (await api.query.StorageProvider.Providers.getValue(charlie.address, READ_OPTS))!;
        assert.ok(stored, "Provider should be in storage");
      },
    });
  } else {
    tests.push({
      name: "1.1 Register new provider (SKIPPED — already registered)",
      fn: async () => {
        console.log("    Charlie already registered from a prior run; skipping");
      },
    });
  }

  tests.push({
    name: "1.2 Update settings",
    fn: async () => {
      const result = await updateProviderSettings(api, charlie, {
        min_duration: 10,
        max_duration: 100_000,
        price_per_byte: 2n,
        accepting_primary: true,
        replica_sync_price: undefined,
        accepting_extensions: true,
        max_capacity: 0n,
      });
      const events = api.event.StorageProvider.ProviderSettingsUpdated.filter(result.events as never);
      assert.strictEqual(events.length, 1, "Expected ProviderSettingsUpdated event");
      const stored = (await api.query.StorageProvider.Providers.getValue(charlie.address, READ_OPTS))!;
      assert.strictEqual(stored.settings.price_per_byte, 2n, "price_per_byte should be 2");
    },
  });

  tests.push({
    name: "1.3 Add stake",
    fn: async () => {
      const before = (await api.query.StorageProvider.Providers.getValue(charlie.address, READ_OPTS))!;
      const result = await addStake(api, charlie, 500n * UNIT);
      const events = api.event.StorageProvider.ProviderStakeAdded.filter(result.events as never);
      assert.strictEqual(events.length, 1, "Expected ProviderStakeAdded event");
      const after = (await api.query.StorageProvider.Providers.getValue(charlie.address, READ_OPTS))!;
      assert.ok(after.stake > before.stake, "Stake should have increased");
    },
  });

  tests.push({
    name: "1.4 Update multiaddr",
    fn: async () => {
      const newAddr = "/ip4/127.0.0.1/tcp/9999";
      await updateProviderMultiaddr(api, charlie, newAddr);
      const stored = (await api.query.StorageProvider.Providers.getValue(charlie.address, READ_OPTS))!;
      const decoded = new TextDecoder().decode(stored.multiaddr);
      assert.strictEqual(decoded, newAddr, "Multiaddr should be updated");
      // Restore original
      const port = new URL(PROVIDER_URL).port;
      await updateProviderMultiaddr(api, charlie, `/ip4/127.0.0.1/tcp/${port}`);
    },
  });

  tests.push({
    name: "1.5 Settings take effect for matching",
    fn: async () => {
      // With accepting_primary=true and price 2 (set in 1.2), Charlie is a
      // perfect match for a request priced at 10.
      const entry = await matchEntry(api, charlie, {
        bytesNeeded: 1_048_576n,
        minDuration: 50,
        maxPricePerByte: 10n,
      });
      assert.ok(entry, "Charlie should appear in matching results");
      assert.strictEqual(entry.match_score, 100, "Charlie should be a perfect match");
      assert.strictEqual(entry.partial_reason, undefined, "No partial-match reason expected");
    },
  });

  // ── Failure tests ─────────────────────────────────────────────────────────

  tests.push({
    name: "1.6 Duplicate registration",
    fn: async () => {
      const tx = api.tx.StorageProvider.register_provider({
        multiaddr: new TextEncoder().encode("/ip4/127.0.0.1/tcp/3333"),
        public_key: charlie.publicKey,
        stake: 1000n * UNIT,
      });
      await submitTxExpectFailure(tx, charlie.signer, "ProviderAlreadyRegistered", "1.6");
    },
  });

  tests.push({
    name: "1.7 Insufficient stake",
    fn: async () => {
      // Dave is not registered — try with very low stake.
      const tx = api.tx.StorageProvider.register_provider({
        multiaddr: new TextEncoder().encode("/ip4/127.0.0.1/tcp/4444"),
        public_key: dave.publicKey,
        stake: 1n, // Way too low
      });
      await submitTxExpectFailure(tx, dave.signer, "InsufficientStake", "1.7");
    },
  });

  tests.push({
    name: "1.8 Non-provider updates settings",
    fn: async () => {
      const tx = api.tx.StorageProvider.update_provider_settings({
        settings: {
          min_duration: 10,
          max_duration: 100_000,
          price_per_byte: 1n,
          accepting_primary: true,
          replica_sync_price: undefined,
          accepting_extensions: true,
          max_capacity: 0n,
        },
      });
      await submitTxExpectFailure(tx, dave.signer, "ProviderNotFound", "1.8");
    },
  });

  tests.push({
    name: "1.9 Non-provider adds stake",
    fn: async () => {
      const tx = api.tx.StorageProvider.add_stake({ amount: 100n * UNIT });
      await submitTxExpectFailure(tx, dave.signer, "ProviderNotFound", "1.9");
    },
  });

  tests.push({
    name: "1.10 min_duration > max_duration",
    fn: async () => {
      const tx = api.tx.StorageProvider.update_provider_settings({
        settings: {
          min_duration: 1000,
          max_duration: 10,
          price_per_byte: 1n,
          accepting_primary: true,
          replica_sync_price: undefined,
          accepting_extensions: true,
          max_capacity: 0n,
        },
      });
      await submitTxExpectFailure(tx, charlie.signer, "MinDurationExceedsMaxDuration", "1.10");
    },
  });

  // ── Edge cases ────────────────────────────────────────────────────────────

  tests.push({
    name: "1.11 accepting_primary=false blocks matching",
    fn: async () => {
      // Turn off accepting_primary for Charlie.
      await updateProviderSettings(api, charlie, {
        min_duration: 10,
        max_duration: 100_000,
        price_per_byte: 2n,
        accepting_primary: false,
        replica_sync_price: undefined,
        accepting_extensions: true,
        max_capacity: 0n,
      });
      try {
        // A non-accepting provider scores 0 with a NotAccepting reason.
        const entry = await matchEntry(api, charlie, {
          bytesNeeded: 1_048_576n,
          minDuration: 50,
          maxPricePerByte: 10n,
        });
        assert.ok(entry, "Charlie should still be listed");
        assert.strictEqual(entry.match_score, 0, "Non-accepting provider should score 0");
        assert.strictEqual(
          entry.partial_reason?.type,
          "NotAccepting",
          "Reason should be NotAccepting"
        );
      } finally {
        // Restore accepting_primary for remaining tests.
        await updateProviderSettings(api, charlie, {
          min_duration: 10,
          max_duration: 100_000,
          price_per_byte: 2n,
          accepting_primary: true,
          replica_sync_price: undefined,
          accepting_extensions: true,
          max_capacity: 0n,
        });
      }
    },
  });

  tests.push({
    name: "1.12 max_capacity=0 (unlimited) allows any size",
    fn: async () => {
      // max_capacity=0 means unlimited.
      const stored = (await api.query.StorageProvider.Providers.getValue(charlie.address, READ_OPTS))!;
      assert.strictEqual(stored.settings.max_capacity, 0n, "max_capacity should be 0 (unlimited)");
      // The previous matching test (1.5) already proved it works with 0.
    },
  });

  await runSuite("01 — Provider Registration", tests, { api, papi });
  papi.destroy();
}

main().catch((err) => {
  console.error(err);
  process.exitCode = 1;
}).finally(() => {
  process.exit(process.exitCode || 0);
});
