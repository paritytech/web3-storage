/**
 * Demo/E2E orchestration helpers that are NOT SDK material: CLI argument
 * parsing, console pretty-printing, and dev-key-based test choreography.
 * Scenario scripts import these from here; everything chain-generic lives in
 * @web3-storage/sdk.
 */

import { READ_OPTS, type ParachainApi } from "@web3-storage/sdk";

/**
 * Default CLI arguments shared by examples that talk to both the chain and
 * the provider node. Layout: `node script.js [chain_ws] [provider_url]
 * [provider_seed] [client_seed]`.
 *
 * Centralised here so every example reads the same defaults — change them
 * once and all the demos pick them up.
 */
export function parseProviderClientArgs(argv = process.argv) {
  return {
    chainWs: argv[2] || "ws://127.0.0.1:2222",
    providerUrl: argv[3] || "http://127.0.0.1:3333",
    providerSeed: argv[4] || "//Alice",
    clientSeed: argv[5] || "//Bob",
  };
}

export function fmtRole(role: unknown): string {
  if (!role) return "?";
  if (typeof role === "string") return role;
  return (role as { type?: string }).type ?? JSON.stringify(role);
}

export async function printBucketMembers(api: ParachainApi, bucketId: bigint, label = "members") {
  const bucket = (await api.query.StorageProvider.Buckets.getValue(
    bucketId,
    READ_OPTS
  ))!;
  console.log(`  [${label}] bucket ${bucketId}:`);
  for (const m of bucket.members) {
    console.log(`    - ${m.account}  role=${fmtRole(m.role)}`);
  }
  return bucket.members;
}

// Signs for arbitrary dev keys — lives with the other test-only powers.
export { ensureSoleAcceptingProvider } from "@web3-storage/test-helpers";
