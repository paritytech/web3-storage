import { getApi, submitExtrinsic } from "./chain-api";
import type { DevSigner } from "./signers";

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
  await submitExtrinsic(
    api.tx.StorageProvider.deregister_provider(),
    account.signer,
  );
}
