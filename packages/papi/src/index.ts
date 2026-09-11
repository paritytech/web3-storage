// SPDX-License-Identifier: Apache-2.0

/**
 * Shared PAPI utilities used across all UIs.
 */

import { ss58Address, ss58Decode } from '@polkadot-labs/hdkd-helpers'
import { multiaddrToUri } from "@multiformats/multiaddr-to-uri";
import { parachain } from "@polkadot-api/descriptors";
import { type TypedApi } from "polkadot-api";
import { fromHex, toHex } from "@polkadot-api/utils";

/** Typed parachain API, shared by every UI that talks to the chain. */
export type ParachainApi = TypedApi<typeof parachain>;

/**
 * Hex helpers re-exported from `@polkadot-api/utils` so UIs have one import
 * site: `toHex` emits a `0x`-prefixed string, `fromHex` accepts hex with or
 * without the prefix.
 */
export { fromHex, toHex };

/**
 * SS58 address prefix used to encode public keys for display.
 *
 * Defaults to 0 (Polkadot `1…` style). The prefix is a per-network property, so
 * a UI that connects to a chain refines it from the runtime on connect via
 * `setSs58Prefix(api.constants.System.SS58Prefix())`. Every address *comparison*
 * goes through `isSameAddress` below (raw bytes), so this value only affects how
 * addresses are rendered — never matching correctness.
 */
let ss58Prefix = 0

/** Current SS58 prefix used by `toSs58`. */
export function getSs58Prefix(): number {
  return ss58Prefix
}

/** Set the SS58 prefix, typically from the connected runtime's `SS58Prefix`. */
export function setSs58Prefix(prefix: number): void {
  ss58Prefix = prefix
}

/** Encode a raw public key as an SS58 address using the current prefix. */
export function toSs58(publicKey: Uint8Array): string {
  return ss58Address(publicKey, ss58Prefix)
}

/** Compare two SS58 addresses by raw public key bytes (prefix-agnostic). */
export function isSameAddress(a: string, b: string): boolean {
  try {
    const [aBytes] = ss58Decode(a)
    const [bBytes] = ss58Decode(b)
    if (aBytes.length !== bBytes.length) return false
    for (let i = 0; i < aBytes.length; i++) {
      if (aBytes[i] !== bBytes[i]) return false
    }
    return true
  } catch {
    return false
  }
}

/**
 * Resolve a libp2p multiaddr string to an HTTP(S) base URL, or `null` if it
 * does not describe one.
 *
 * Delegates to `@multiformats/multiaddr-to-uri` so we speak the same multiaddr
 * grammar as the rest of the ecosystem (`tls`/`https`, `http-path`, `sni`,
 * default-port elision, …). The provider node registers plain
 * `/ip4/<host>/tcp/<port>` for local dev and
 * `/dns4/<host>/tcp/443/tls/http/http-path/<path>` for TLS-terminated hosted
 * deployments; both round-trip through this function:
 * - `/ip4/127.0.0.1/tcp/3333` → `http://127.0.0.1:3333`
 * - `/dns4/host/tcp/443/tls/http/http-path/web3-storage-provider`
 *   → `https://host/web3-storage-provider`
 *
 * `multiaddrToUri` throws on malformed input and can emit non-HTTP schemes
 * (e.g. `tcp://` for a bare `/tls` or a `/p2p`-terminated address) or a
 * scheme-less host; we treat all of those as "not an HTTP endpoint" and return
 * `null` so callers fall through to the next provider.
 */
export function parseMultiaddrToUrl(multiaddr: string): string | null {
  let uri: string;
  try {
    uri = multiaddrToUri(multiaddr);
  } catch {
    return null;
  }
  return /^https?:\/\//.test(uri) ? uri : null;
}

/**
 * Provider-signed agreement terms returned by `POST /negotiate` on the
 * provider node. The signature is the SCALE-encoded `MultiSignature` as
 * hex (e.g. `0x01<64-byte-sr25519-sig>`).
 */
export interface SignedTerms {
  terms: {
    owner: string;
    max_bytes: number | bigint;
    duration: number;
    price_per_byte: number | bigint;
    valid_until: number;
    nonce: number | bigint;
    replica_params: unknown | null;
    bucket_id: bigint | null;
  };
  signature: string;
}

/** Body of a `POST /negotiate` request to a provider node. */
export interface NegotiateRequest {
  owner: string;
  max_bytes: number | bigint;
  duration: number;
  price_per_byte: number | bigint;
  replica_params: unknown | null;
  bucket_id?: bigint | null;
}

/**
 * POST a `NegotiateRequest` to the provider's `/negotiate` endpoint and
 * return the provider-signed terms bundle.
 */
export async function negotiateTerms(
  providerUrl: string,
  request: NegotiateRequest,
): Promise<SignedTerms> {
  const res = await fetch(`${providerUrl.replace(/\/$/, "")}/negotiate`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(request, (_k, v) => (typeof v === "bigint" ? v.toString() : v)),
  });
  if (!res.ok) {
    const body = await res.text().catch(() => "");
    throw new Error(`/negotiate failed: ${res.status} ${body}`);
  }
  return res.json();
}

/** Outcome of {@link negotiateProviderTerms}: resolved endpoint + terms, or a UI-ready error. */
export type NegotiateProviderResult =
  | { ok: true; url: string; signed: SignedTerms }
  | { ok: false; error: string };

/**
 * Resolve a provider's multiaddr to a base URL and negotiate signed terms in
 * one step — the shared core of the "create bucket" / "create drive" flows.
 *
 * Returns a discriminated result instead of throwing so callers can surface the
 * message inline: a `null` multiaddr and a failed `/negotiate` both come back as
 * `{ ok: false, error }`.
 */
export async function negotiateProviderTerms(
  provider: { account: string; multiaddr: string },
  request: NegotiateRequest,
): Promise<NegotiateProviderResult> {
  const url = parseMultiaddrToUrl(provider.multiaddr);
  if (!url) {
    return {
      ok: false,
      error: `Provider ${provider.account} has an unparseable multiaddr: ${provider.multiaddr}`,
    };
  }
  try {
    const signed = await negotiateTerms(url, request);
    return { ok: true, url, signed };
  } catch (err) {
    return {
      ok: false,
      error: err instanceof Error ? err.message : "Failed to negotiate with provider",
    };
  }
}

const HTTP_RETRY_ATTEMPTS = 3;
const HTTP_RETRY_BASE_MS = 250;

function sleep(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms));
}

function isAbortError(err: unknown): boolean {
  return (
    err instanceof DOMException &&
    (err.name === "AbortError" || err.code === DOMException.ABORT_ERR)
  );
}

function isRetryableHttpError(status: number | null): boolean {
  if (status === null) return true;
  return status >= 500 && status < 600;
}

/**
 * `fetch` with bounded exponential-backoff retry on transient failures (5xx and
 * network errors). Aborts (via `init.signal`) propagate immediately and are
 * never retried.
 */
export async function httpFetch(
  url: string,
  init: RequestInit & { signal?: AbortSignal } = {},
): Promise<Response> {
  let lastError: unknown = null;
  for (let attempt = 0; attempt < HTTP_RETRY_ATTEMPTS; attempt++) {
    try {
      const res = await fetch(url, init);
      if (res.ok || !isRetryableHttpError(res.status)) return res;
      lastError = new Error(`HTTP ${res.status}: ${await res.text().catch(() => "")}`);
    } catch (err) {
      if (isAbortError(err)) throw err;
      lastError = err;
    }
    if (attempt < HTTP_RETRY_ATTEMPTS - 1) {
      await sleep(HTTP_RETRY_BASE_MS * Math.pow(2, attempt));
    }
  }
  throw lastError instanceof Error ? lastError : new Error("HTTP request failed");
}

/**
 * Resolve the HTTP(S) endpoint for a bucket's primary provider purely from
 * on-chain data: read the bucket, walk its primary providers, and return the
 * first one whose registered multiaddr parses to an HTTP(S) URL.
 *
 * Throws when the bucket is missing, has no primary providers, or none of them
 * expose a usable HTTP endpoint. Callers that want a dev fallback (e.g. local
 * `http://127.0.0.1:3333`) should layer it on top of this.
 */
export async function resolveProviderEndpoint(
  api: ParachainApi,
  bucketId: bigint,
  // Finalized by default (UI-grade). Callers that just observed an acceptance
  // at the best head (e.g. watch-based waits) pass { at: "best" } so the
  // resolution can't lag the event that triggered it.
  readOpts: { at: "best" | "finalized" } = { at: "finalized" },
): Promise<string> {
  const bucket = await api.query.StorageProvider.Buckets.getValue(bucketId, readOpts);
  if (!bucket) throw new Error(`Bucket ${bucketId} not found on chain`);

  const providers: string[] = bucket.primary_providers ?? [];
  if (providers.length === 0) {
    throw new Error(`Bucket ${bucketId} has no primary providers`);
  }

  for (const providerAccount of providers) {
    const provider = await api.query.StorageProvider.Providers.getValue(providerAccount, readOpts);
    if (!provider) continue;

    // multiaddr is a BoundedVec<u8> — decode to string.
    const multiaddrStr = new TextDecoder().decode(provider.multiaddr);
    const url = parseMultiaddrToUrl(multiaddrStr);
    if (url) return url;
  }

  throw new Error(`Could not resolve HTTP endpoint for bucket ${bucketId} providers`);
}
