import { getApi, submitExtrinsic, submitExtrinsicBestBlock } from "./chain-api";
import { devSigners, type DevAccountName, type DevSigner } from "./signers";

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
 * deregister anyway) and skips registrations whose signer we don't have
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
  const keep = new Set(keepAccounts.map((a) => a.address));
  const knownDev: Record<string, DevSigner> = {};
  for (const name of Object.keys(devSigners) as DevAccountName[]) {
    knownDev[devSigners[name].address] = devSigners[name];
  }

  const entries = await api.query.StorageProvider.Providers.getEntries();
  for (const { keyArgs, value } of entries) {
    const address = keyArgs[0] as string;
    if (keep.has(address)) continue;
    const signer = knownDev[address];
    if (!signer) continue;
    if ((value as { committed_bytes: bigint }).committed_bytes !== 0n) continue;
    await submitExtrinsicBestBlock(
      api.tx.StorageProvider.deregister_provider(),
      signer.signer,
    );
  }
}
