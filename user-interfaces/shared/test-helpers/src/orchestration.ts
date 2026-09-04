// SPDX-License-Identifier: GPL-3.0-only

/**
 * Cross-suite test choreography that signs for arbitrary dev keys — test-only
 * power, deliberately outside @web3-storage/sdk. Consumed by the PAPI E2E
 * suite (examples/papi) and available to Playwright global setups.
 */

import {
  devSigners,
  makeSigner,
  READ_OPTS,
  sameAddress,
  submitTx,
  type ChainSigner,
  type ParachainApi,
} from "@web3-storage/sdk";

const KNOWN_DEV_SEEDS = Object.values(devSigners).map((s) => s.seed);

/**
 * Make `keep` the only provider that will be picked by auto-matching
 * extrinsics (`establish_storage_agreement`, `create_s3_bucket`,
 * `create_drive`).
 *
 * The Layer 1 paths select via `query_available_providers[0]`, which iterates
 * `Providers` in storage-hash order — non-deterministic across AccountIds.
 * When CI registers a second provider, the auto-match flips between them at
 * random and tests that assume a specific provider signed the checkpoint
 * fail intermittently with `ProviderNotInSnapshot` or
 * `AgreementRequestNotFound`.
 *
 * Iterates the known dev seeds, finds any provider that is currently
 * registered and `accepting_primary`, and (if it isn't the keep target)
 * flips `accepting_primary` to false. Returns an async `restore` function
 * that puts each toggled provider back to its original settings.
 *
 * Throws if an unknown (non-dev-key) provider is accepting — we can't sign
 * for it, so determinism can't be guaranteed and the caller should learn
 * about that explicitly rather than flake later.
 */
export async function ensureSoleAcceptingProvider(
  api: ParachainApi,
  keep: ChainSigner,
): Promise<() => Promise<void>> {
  const toggled: Array<{ signer: ChainSigner; original: unknown; seed: string }> = [];
  const others = await api.query.StorageProvider.Providers.getEntries(READ_OPTS);
  for (const { keyArgs, value: info } of others) {
    const account = keyArgs[0];
    if (sameAddress(account, keep.address)) continue;
    if (!info.settings.accepting_primary) continue;
    const seed = KNOWN_DEV_SEEDS.find((s) => sameAddress(makeSigner(s).address, account));
    if (!seed) {
      throw new Error(
        `Provider ${account} is registered with accepting_primary=true but ` +
          `is not a known dev key — cannot silence it to make auto-matching ` +
          `deterministic. Add its seed to the dev signers or stop the test.`,
      );
    }
    const signer = makeSigner(seed);
    const original = info.settings;
    await submitTx(
      api.tx.StorageProvider.update_provider_settings({
        settings: { ...original, accepting_primary: false },
      }),
      signer.signer,
      `disable accepting_primary for ${seed}`,
    );
    toggled.push({ signer, original, seed });
  }
  return async function restore() {
    for (const { signer, original, seed } of toggled) {
      await submitTx(
        api.tx.StorageProvider.update_provider_settings({
          settings: original as never,
        }),
        signer.signer,
        `restore accepting_primary for ${seed}`,
      );
    }
  };
}
