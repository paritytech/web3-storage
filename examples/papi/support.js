/**
 * Demo/E2E orchestration helpers that are NOT SDK material: CLI argument
 * parsing, console pretty-printing, and dev-key-based test choreography.
 * Scenario scripts import these from here; everything chain-generic lives in
 * @web3-storage/sdk.
 */

import { makeSigner, READ_OPTS, sameAddress, submitTx } from "@web3-storage/sdk";

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

export function fmtRole(role) {
  if (!role) return "?";
  if (typeof role === "string") return role;
  return role.type ?? JSON.stringify(role);
}

export async function printBucketMembers(api, bucketId, label = "members") {
  const bucket = await api.query.StorageProvider.Buckets.getValue(
    bucketId,
    READ_OPTS
  );
  console.log(`  [${label}] bucket ${bucketId}:`);
  for (const m of bucket.members) {
    console.log(`    - ${m.account}  role=${fmtRole(m.role)}`);
  }
  return bucket.members;
}

/**
 * Dev seeds we know how to sign for. Used by `ensureSoleAcceptingProvider`
 * to silence any non-target provider that may have been registered by an
 * earlier demo on the same chain (e.g. CI starts a second provider node
 * keyed to //Charlie on a different port).
 */
const KNOWN_DEV_SEEDS = [
  "//Alice",
  "//Bob",
  "//Charlie",
  "//Dave",
  "//Eve",
  "//Ferdie",
];

/**
 * Make `keep` the only provider that will be picked by auto-matching
 * extrinsics (`create_bucket_with_storage`, `create_s3_bucket_with_storage`,
 * `create_drive`).
 *
 * The Layer 1 paths select via `query_available_providers[0]`, which iterates
 * `Providers` in storage-hash order — non-deterministic across AccountIds.
 * When CI registers a second provider, the auto-match flips between them at
 * random and demos that assume a specific provider signed the checkpoint
 * fail intermittently with `ProviderNotInSnapshot` or
 * `AgreementRequestNotFound`.
 *
 * This helper iterates the known dev seeds, finds any provider that is
 * currently registered and `accepting_primary`, and (if it isn't the keep
 * target) flips `accepting_primary` to false. Returns an async `restore`
 * function that puts each toggled provider back to its original settings.
 *
 * If an unknown (non-dev-key) provider is accepting, this throws — we
 * can't sign for it, so determinism can't be guaranteed and the caller
 * should learn about that explicitly rather than flake later.
 */
export async function ensureSoleAcceptingProvider(api, keep) {
  const toggled = [];
  const others = await api.query.StorageProvider.Providers.getEntries(READ_OPTS);
  for (const { keyArgs, value: info } of others) {
    const account = keyArgs[0];
    if (sameAddress(account, keep.address)) continue;
    if (!info.settings.accepting_primary) continue;
    const seed = KNOWN_DEV_SEEDS.find((s) =>
      sameAddress(makeSigner(s).address, account)
    );
    if (!seed) {
      throw new Error(
        `Provider ${account} is registered with accepting_primary=true but ` +
          `is not a known dev key — cannot silence it to make auto-matching ` +
          `deterministic. Add its seed to KNOWN_DEV_SEEDS or stop the demo.`
      );
    }
    const signer = makeSigner(seed);
    const original = info.settings;
    await submitTx(
      api.tx.StorageProvider.update_provider_settings({
        settings: { ...original, accepting_primary: false },
      }),
      signer.signer,
      `disable accepting_primary for ${seed}`
    );
    toggled.push({ signer, original, seed });
  }
  return async function restore() {
    for (const { signer, original, seed } of toggled) {
      await submitTx(
        api.tx.StorageProvider.update_provider_settings({ settings: original }),
        signer.signer,
        `restore accepting_primary for ${seed}`
      );
    }
  };
}
