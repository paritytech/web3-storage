/**
 * Shared helpers for the PAPI examples in this directory.
 *
 * Importable from any example via `import { ... } from "./common.js"`.
 */

import { createClient } from "polkadot-api";
import { getWsProvider } from "polkadot-api/ws-provider";
import { getPolkadotSigner } from "polkadot-api/signer";
import { Keyring } from "@polkadot/keyring";
import { cryptoWaitReady, decodeAddress } from "@polkadot/util-crypto";
import { parachain } from "@polkadot-api/descriptors";

export function makeSigner(seed) {
  const keyring = new Keyring({ type: "sr25519" });
  const account = keyring.addFromUri(seed);
  return {
    signer: getPolkadotSigner(account.publicKey, "Sr25519", (input) =>
      account.sign(input)
    ),
    address: account.address,
    publicKey: account.publicKey,
    seed,
  };
}

export function toHex(bytes) {
  const arr = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
  return "0x" + Array.from(arr, (b) => b.toString(16).padStart(2, "0")).join("");
}

export function hexToBytes(hex) {
  const h = hex.startsWith("0x") ? hex.slice(2) : hex;
  const bytes = new Uint8Array(h.length / 2);
  for (let i = 0; i < bytes.length; i++) {
    bytes[i] = parseInt(h.substr(i * 2, 2), 16);
  }
  return bytes;
}

export async function connect(chainWs) {
  await cryptoWaitReady();
  const papi = createClient(getWsProvider(chainWs));
  const api = papi.getTypedApi(parachain);
  return { papi, api };
}

/**
 * Sign + submit a transaction and assert it dispatched successfully.
 *
 * PAPI's bare `signAndSubmit` resolves with `{ ok, events, dispatchError }` and
 * does NOT throw when dispatch fails — only when the tx is invalid (bad
 * signature, low nonce, etc). Without this helper, a failed extrinsic looks
 * indistinguishable from a successful one with no events, and the failure
 * surfaces later as a confusing "Expected exactly 1 X event, got 0".
 */
export async function submitTx(tx, signer, label) {
  const result = await tx.signAndSubmit(signer);
  if (!result.ok) {
    const err = result.dispatchError;
    const detail =
      err && typeof err === "object"
        ? `${err.type ?? "DispatchError"}` +
          (err.value?.type ? `::${err.value.type}` : "") +
          (err.value?.value?.type ? `::${err.value.value.type}` : "")
        : String(err);
    throw new Error(`${label} dispatch failed: ${detail}`);
  }
  return result;
}

export async function providerFetch(providerUrl, path, opts = {}) {
  const url = new URL(path, providerUrl);
  if (opts.params) {
    for (const [k, v] of Object.entries(opts.params))
      url.searchParams.set(k, v);
  }
  const resp = await fetch(url, {
    method: opts.method || "GET",
    headers: opts.body ? { "Content-Type": "application/json" } : undefined,
    body: opts.body ? JSON.stringify(opts.body) : undefined,
  });
  if (!resp.ok) throw new Error(`${path}: ${resp.status} ${await resp.text()}`);
  return resp.json();
}

/**
 * Ensure a provider is registered and configured to accept primary agreements.
 *
 * Re-used by the s3 and drive examples so they can be run standalone (without
 * having to run full-flow.js first to register the provider).
 */
export async function ensureProviderRegistered(api, provider, providerUrl, {
  pricePerByte = 1n,
  maxDuration = 100_000,
} = {}) {
  const existing = await api.query.StorageProvider.Providers.getValue(
    provider.address
  );
  if (!existing) {
    const { Binary } = await import("@polkadot-api/substrate-bindings");
    const port = new URL(providerUrl).port;
    const multiaddr = new TextEncoder().encode(`/ip4/127.0.0.1/tcp/${port}`);
    console.log("  Registering provider", provider.address);
    await submitTx(
      api.tx.StorageProvider.register_provider({
        multiaddr: Binary.fromBytes(multiaddr),
        public_key: Binary.fromBytes(provider.publicKey),
        stake: 1_000_000_000_000_000n,
      }),
      provider.signer,
      "register_provider"
    );
  } else {
    // Provider already registered (likely by an earlier demo on the same
    // chain). Sanity-check that the on-chain public_key matches the keyring's
    // — if it doesn't, off-chain signatures produced by the provider node
    // will fail to verify and the failure surfaces as a missing event much
    // later. Better to fail loudly here.
    const onChainKey = existing.public_key?.asBytes
      ? existing.public_key.asBytes()
      : existing.public_key;
    if (!bytesEq(onChainKey, provider.publicKey)) {
      throw new Error(
        `Provider ${provider.address} is already registered with a different public_key. ` +
          `Restart the chain, or run this script with a fresh provider seed.`
      );
    }
  }
  // Always (re)apply settings so price/acceptance are correct for this demo.
  await submitTx(
    api.tx.StorageProvider.update_provider_settings({
      settings: {
        min_duration: 10,
        max_duration: maxDuration,
        price_per_byte: pricePerByte,
        accepting_primary: true,
        replica_sync_price: undefined,
        accepting_extensions: true,
        max_capacity: 0n,
      },
    }),
    provider.signer,
    "update_provider_settings"
  );
}

function bytesEq(a, b) {
  if (!a || !b || a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
  return true;
}

export function fmtRole(role) {
  if (!role) return "?";
  if (typeof role === "string") return role;
  return role.type ?? JSON.stringify(role);
}

export function bytesToUtf8(maybeBin) {
  if (!maybeBin) return "";
  const bytes = maybeBin.asBytes ? maybeBin.asBytes() : maybeBin;
  return new TextDecoder().decode(bytes);
}

export function utf8(s) {
  return new TextEncoder().encode(s);
}

export async function acceptAgreement(api, provider, bucketId) {
  await submitTx(
    api.tx.StorageProvider.accept_agreement({ bucket_id: bucketId }),
    provider.signer,
    "accept_agreement"
  );
}

/**
 * Wait until the pending AgreementRequest for `(providerAddress, bucketId)`
 * has been consumed — i.e. the provider has accepted it. The provider node's
 * agreement_coordinator polls every ~6s and auto-accepts, which races any
 * explicit `accept_agreement` extrinsic the demo might submit; if the
 * provider wins, the demo's submit fails with `AgreementRequestNotFound`.
 *
 * Polling instead of submitting sidesteps the race entirely.
 */
export async function waitForAgreementAcceptance(
  api,
  providerAddress,
  bucketId,
  { timeoutMs = 60_000, pollMs = 1_000 } = {}
) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const req = await api.query.StorageProvider.AgreementRequests.getValue(
      providerAddress,
      bucketId
    );
    if (!req) return;
    await new Promise((r) => setTimeout(r, pollMs));
  }
  throw new Error(
    `Timed out after ${timeoutMs}ms waiting for provider ${providerAddress} ` +
      `to auto-accept the agreement request for bucket ${bucketId}. ` +
      `Is the provider node's agreement_coordinator running?`
  );
}

export async function printBucketMembers(api, bucketId, label = "members") {
  const bucket = await api.query.StorageProvider.Buckets.getValue(bucketId);
  console.log(`  [${label}] bucket ${bucketId}:`);
  for (const m of bucket.members) {
    console.log(`    - ${m.account}  role=${fmtRole(m.role)}`);
  }
  return bucket.members;
}

/**
 * Compare two SS58 addresses by their underlying public key.
 *
 * PAPI encodes AccountIds with the runtime's SS58 prefix (Polkadot-style
 * `1…` strings on this parachain), while @polkadot/keyring uses the
 * substrate-default prefix 42 (`5…` strings). Same key, different string,
 * so string equality fails.
 */
export function sameAddress(a, b) {
  try {
    const aBytes = decodeAddress(a);
    const bBytes = decodeAddress(b);
    if (aBytes.length !== bBytes.length) return false;
    for (let i = 0; i < aBytes.length; i++) {
      if (aBytes[i] !== bBytes[i]) return false;
    }
    return true;
  } catch {
    return false;
  }
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
  const others = await api.query.StorageProvider.Providers.getEntries();
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

export function requireOneEvent(events, eventDescriptor, name) {
  const matched = eventDescriptor.filter(events);
  if (matched.length !== 1) {
    throw new Error(
      `Expected exactly 1 ${name} event, got ${matched.length}`
    );
  }
  return matched[0];
}
