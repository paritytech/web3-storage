// SPDX-License-Identifier: GPL-3.0-only

import { Blake2128Concat, Twox128 } from "@polkadot-api/substrate-bindings";
import { ss58Decode } from "@polkadot-labs/hdkd-helpers";
import { getApi, submitExtrinsic, submitExtrinsicBestBlock } from "./chain-api";
import { Alice, devSigners, type DevAccountName, type DevSigner } from "./signers";

/**
 * Hex-encode an account's raw 32-byte public key. PAPI returns
 * `getEntries()` keys in the chain's SS58 prefix (42 on the local
 * runtime, 0 on paseo) while `signers.ts` builds dev-signer addresses
 * with the shared default prefix (42) — string equality then fails on
 * paseo. Compare via raw bytes so the helper works against either runtime.
 */
function publicKeyHex(address: string): string {
  const [bytes] = ss58Decode(address);
  return Array.from(bytes)
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
}

/**
 * Storage key for `pallet_storage_provider::Providers[address]`. Layout
 * is `twox128(pallet_name) ++ twox128(storage_name) ++
 * blake2_128_concat(scale(account))`, matching the FRAME `StorageMap`
 * with `Blake2_128Concat` hasher in `pallets/storage-provider/src/lib.rs`. AccountId32 is
 * 32 raw bytes, so the scale encoding of the key is just those bytes.
 */
function providersStorageKey(address: string): Uint8Array {
  const utf8 = new TextEncoder();
  const [accountBytes] = ss58Decode(address);
  const palletPrefix = Twox128(utf8.encode("StorageProvider"));
  const storagePrefix = Twox128(utf8.encode("Providers"));
  const keyTail = Blake2128Concat(accountBytes);
  const key = new Uint8Array(palletPrefix.length + storagePrefix.length + keyTail.length);
  key.set(palletPrefix, 0);
  key.set(storagePrefix, palletPrefix.length);
  key.set(keyTail, palletPrefix.length + storagePrefix.length);
  return key;
}

/**
 * Remove `account`'s `Providers` entry directly via
 * `Sudo.sudo(System.kill_storage(...))`. The runtime's deregister is a
 * 2-step lifecycle gated by `DeregisterAnnouncementPeriod` (48h on this
 * runtime) — there's no live-chain extrinsic that can fully unregister
 * a provider within a test run. Wiping the storage entry by key from
 * sudo bypasses that lifecycle entirely, matching the spirit of the
 * runtime integration test's `set_block_number` + `complete_deregister`
 * sequence in `should_deregister_provider`.
 *
 * Stake stays reserved on `account`'s balance (kill_storage skips the
 * unreserve `complete_deregister` would do). Dev accounts are funded
 * with 10^25 units in genesis, so this is fine for test cycles.
 */
async function forceRemoveProvider(account: DevSigner): Promise<void> {
  const api = getApi();
  const existing = await api.query.StorageProvider.Providers.getValue(account.address);
  if (!existing) return;
  if (existing.committed_bytes !== 0n) {
    throw new Error(
      `forceRemoveProvider: ${account.name} has committed_bytes=${existing.committed_bytes}; refusing to wipe a provider with active agreements`,
    );
  }

  const key = providersStorageKey(account.address);
  const killStorage = api.tx.System.kill_storage({ keys: [key] });
  await submitExtrinsic(
    api.tx.Sudo.sudo({ call: killStorage.decodedCall }),
    Alice.signer,
  );
}

export interface ProviderSettings {
  min_duration: number;
  max_duration: number;
  price_per_byte: bigint;
  accepting_primary: boolean;
  replica_sync_price: bigint | undefined;
  accepting_extensions: boolean;
  max_capacity: bigint;
}

export interface RegistrationOptions {
  multiaddr?: string;
  stake?: bigint;
  settings?: Partial<ProviderSettings>;
}

const DEFAULT_MULTIADDR = "/ip4/127.0.0.1/tcp/3333";
const DEFAULT_STAKE = 1_000_000_000_000_000n; // 1000 tokens (12 decimals)

const DEFAULT_SETTINGS: ProviderSettings = {
  min_duration: 10,
  max_duration: 100_000,
  price_per_byte: 1n,
  accepting_primary: true,
  replica_sync_price: undefined,
  accepting_extensions: true,
  max_capacity: 0n,
};

export interface RegistrationResult {
  alreadyRegistered: boolean;
  address: string;
}

/**
 * Register a provider via direct extrinsic. Idempotent — if `account` is
 * already registered, returns `alreadyRegistered: true` without re-submitting.
 *
 * Always (re-)applies the requested settings, since a re-run may want to
 * change pricing/capacity from a prior test's leftovers.
 */
export async function registerProviderViaApi(
  account: DevSigner,
  opts: RegistrationOptions = {},
): Promise<RegistrationResult> {
  const api = getApi();
  const existing = await api.query.StorageProvider.Providers.getValue(account.address);

  if (!existing) {
    const multiaddrBytes = new TextEncoder().encode(opts.multiaddr ?? DEFAULT_MULTIADDR);
    await submitExtrinsic(
      api.tx.StorageProvider.register_provider({
        multiaddr: multiaddrBytes,
        public_key: account.publicKey,
        stake: opts.stake ?? DEFAULT_STAKE,
      }),
      account.signer,
    );
  }

  const settings: ProviderSettings = { ...DEFAULT_SETTINGS, ...opts.settings };
  await submitExtrinsic(
    api.tx.StorageProvider.update_provider_settings({ settings }),
    account.signer,
  );

  return { alreadyRegistered: !!existing, address: account.address };
}

export async function isProviderRegistered(address: string): Promise<boolean> {
  const api = getApi();
  const existing = await api.query.StorageProvider.Providers.getValue(address);
  return !!existing;
}

/**
 * Deregister `account` if currently registered AND has no committed bytes
 * (= no active agreements). No-op otherwise. Useful in test globalSetup to
 * reset accounts that are exercised by registration-wizard tests so the
 * wizard test can run on every suite invocation, not just against a fresh
 * chain.
 */
export async function deregisterProviderViaApi(account: DevSigner): Promise<void> {
  const api = getApi();
  const existing = await api.query.StorageProvider.Providers.getValue(account.address);
  if (!existing) return;
  if (existing.committed_bytes !== 0n) {
    throw new Error(
      `deregisterProviderViaApi: ${account.name} has committed_bytes=${existing.committed_bytes}; deregister rejected by runtime`,
    );
  }
  await submitExtrinsicBestBlock(
    api.tx.StorageProvider.deregister_provider(),
    account.signer,
  );
}

/**
 * Deregister every dev-known provider whose account is NOT in `keepAccounts`.
 * Skips registrations with committed_bytes > 0 (runtime would reject the
 * deregister anyway), registrations already in the deregister-announcement
 * window (`deregister_at` set; the runtime rejects a second announce with
 * `DeregisterAnnounced`), and registrations whose signer we don't have
 * (non-dev accounts the test infra didn't create).
 *
 * Why this exists: the runtime's drive-creation flow picks
 * `available_primary_providers[0]` (storage-map iteration order, hash-based)
 * to target with the agreement request. Across test runs, suites accumulate
 * provider registrations from prior runs (e.g. provider-e2e's wizard
 * registers Ferdie, drive-ui keeps Alice, etc.), and the runtime may pick a
 * provider that has no node behind it — the agreement request then sits
 * unaccepted and `createDriveViaApi` times out. Calling this in each
 * suite's globalSetup ensures only the providers a suite actually needs
 * survive on chain when its tests run.
 */
export async function cleanProviderRegistry(
  keepAccounts: DevSigner[],
): Promise<void> {
  const api = getApi();
  const keep = new Set(keepAccounts.map((a) => publicKeyHex(a.address)));
  const knownDev: Record<string, DevSigner> = {};
  for (const name of Object.keys(devSigners) as DevAccountName[]) {
    knownDev[publicKeyHex(devSigners[name].address)] = devSigners[name];
  }

  const entries = await api.query.StorageProvider.Providers.getEntries();
  for (const { keyArgs, value } of entries) {
    const pk = publicKeyHex(keyArgs[0] as string);
    if (keep.has(pk)) continue;
    const signer = knownDev[pk];
    if (!signer) continue;
    const info = value as {
      committed_bytes: bigint;
      settings: { accepting_primary: boolean } & Record<string, unknown>;
    };
    if (info.committed_bytes !== 0n) {
      // Can't wipe a provider with active agreements — but it can still be
      // picked by auto-matching and stall every drive/bucket creation (its
      // node isn't running). Silence it instead: flip accepting_primary off.
      if (info.settings.accepting_primary) {
        await submitExtrinsic(
          api.tx.StorageProvider.update_provider_settings({
            settings: { ...info.settings, accepting_primary: false } as never,
          }),
          signer.signer,
        );
      }
      continue;
    }
    await forceRemoveProvider(signer);
  }
}
