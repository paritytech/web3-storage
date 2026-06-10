/**
 * Provider discovery / marketplace example.
 *
 * Walks the StorageProvider.Providers storage map, optionally filters by a
 * user-supplied set of storage requirements, scores each provider, and prints
 * a ranked list — a JS mirror of the DiscoveryClient SDK feature.
 *
 * This is a pure read-only example: no transactions are submitted and no
 * provider node is contacted. Useful as a starting point for building a
 * marketplace UI on top of the chain.
 *
 * Prerequisites:
 *   - Parachain running at ws://127.0.0.1:2222
 *   - At least one provider registered (run full-flow.js once to set up Alice)
 *
 * Usage:
 *   node provider-discovery.js [chain_ws] [bytes_needed] [duration_blocks] [max_price_per_byte]
 *
 *   bytes_needed       - bytes the client wants to store (default: 1 GiB)
 *   duration_blocks    - desired agreement duration (default: 100)
 *   max_price_per_byte - max acceptable price per byte per block (default: 10)
 */

import {
  connect,
  READ_OPTS,
  waitForBlockProduction,
  waitForChainReady,
  waitForNextBlock,
} from "@web3-storage/sdk";

const CHAIN_WS = process.argv[2] || "ws://127.0.0.1:2222";
const BYTES_NEEDED = BigInt(process.argv[3] || 1_073_741_824n); // 1 GiB
const DURATION = Number(process.argv[4] || 100);
const MAX_PRICE_PER_BYTE = BigInt(process.argv[5] || 10n);

function scoreProvider(info, req) {
  if (!info.settings.accepting_primary) {
    return { score: 0, reasons: ["not accepting primary"] };
  }

  let score = 100;
  const reasons = [];

  const capacity = info.settings.max_capacity;
  const free = capacity === 0n ? null : capacity - info.committed_bytes;
  if (free !== null && free < req.bytesNeeded) {
    score -= 50;
    reasons.push(`free=${free} < needed=${req.bytesNeeded}`);
  }

  if (info.settings.price_per_byte > req.maxPricePerByte) {
    score -= 30;
    reasons.push(
      `price=${info.settings.price_per_byte} > max=${req.maxPricePerByte}`
    );
  }

  const dur = BigInt(req.duration);
  if (
    BigInt(info.settings.min_duration) > dur ||
    BigInt(info.settings.max_duration) < dur
  ) {
    score -= 20;
    reasons.push(
      `duration ${req.duration} not in [${info.settings.min_duration}, ${info.settings.max_duration}]`
    );
  }

  return { score, reasons };
}

async function fetchAndRankProviders(api, req) {
  const entries = await api.query.StorageProvider.Providers.getEntries(READ_OPTS);
  const ranked = entries.map(({ keyArgs, value }) => ({
    address: keyArgs[0],
    info: value,
    ...scoreProvider(value, req),
  }));
  ranked.sort((a, b) => b.score - a.score);
  return ranked;
}

function printProvider({ address, info, score, reasons }) {
  const cap = info.settings.max_capacity;
  const free = cap === 0n ? "unlimited" : (cap - info.committed_bytes).toString();
  console.log(`  ${address}`);
  console.log(`    score              = ${score}`);
  console.log(`    multiaddr          = ${info.multiaddr.asText()}`);
  console.log(`    stake              = ${info.stake}`);
  console.log(`    price_per_byte     = ${info.settings.price_per_byte}`);
  console.log(
    `    duration window    = [${info.settings.min_duration}, ${info.settings.max_duration}]`
  );
  console.log(`    accepting_primary  = ${info.settings.accepting_primary}`);
  console.log(`    committed_bytes    = ${info.committed_bytes}`);
  console.log(
    `    max_capacity       = ${cap === 0n ? "0 (unlimited)" : cap.toString()}`
  );
  console.log(`    free_capacity      = ${free}`);
  if (reasons.length > 0) console.log(`    deductions         = ${reasons.join("; ")}`);
  console.log();
}

async function main() {
  const req = {
    bytesNeeded: BYTES_NEEDED,
    duration: DURATION,
    maxPricePerByte: MAX_PRICE_PER_BYTE,
  };

  console.log("Chain:", CHAIN_WS);
  console.log("Requirements:");
  console.log("  bytes_needed       =", req.bytesNeeded);
  console.log("  duration_blocks    =", req.duration);
  console.log("  max_price_per_byte =", req.maxPricePerByte);

  const { papi, api } = await connect(CHAIN_WS);
  await waitForChainReady(api);
  await waitForBlockProduction(api);
  await waitForNextBlock(papi);

  try {
    const ranked = await fetchAndRankProviders(api, req);
    console.log("\nFound %d registered provider(s).", ranked.length);
    if (ranked.length === 0) {
      console.log("No providers on chain. Run full-flow.js to register one.");
      return;
    }

    console.log("\nRanked providers:\n");
    for (const p of ranked) printProvider(p);

    const eligible = ranked.filter((p) => p.score === 100);
    console.log("Fully-matching providers (score=100): %d", eligible.length);
  } catch (err) {
    console.error("\nERROR:", err.message || err);
    if (err.stack) console.error(err.stack);
    process.exitCode = 1;
  } finally {
    papi.destroy();
  }
}

main().then(() => console.log("=== Done ==="));
