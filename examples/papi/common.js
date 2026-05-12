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
    await api.tx.StorageProvider.register_provider({
      multiaddr: Binary.fromBytes(multiaddr),
      public_key: Binary.fromBytes(provider.publicKey),
      stake: 1_000_000_000_000_000n,
    }).signAndSubmit(provider.signer);
  }
  // Always (re)apply settings so price/acceptance are correct for this demo.
  await api.tx.StorageProvider.update_provider_settings({
    settings: {
      min_duration: 10,
      max_duration: maxDuration,
      price_per_byte: pricePerByte,
      accepting_primary: true,
      replica_sync_price: undefined,
      accepting_extensions: true,
      max_capacity: 0n,
    },
  }).signAndSubmit(provider.signer);
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
  await api.tx.StorageProvider.accept_agreement({
    bucket_id: bucketId,
  }).signAndSubmit(provider.signer);
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

export function requireOneEvent(events, eventDescriptor, name) {
  const matched = eventDescriptor.filter(events);
  if (matched.length !== 1) {
    throw new Error(
      `Expected exactly 1 ${name} event, got ${matched.length}`
    );
  }
  return matched[0];
}
