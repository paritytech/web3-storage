// SPDX-License-Identifier: Apache-2.0

/**
 * Provider-node HTTP plumbing: fetch with retry/backoff + AbortSignal, and
 * the signed Authorization header the provider verifies in auth.rs.
 */

import { toHex } from "./bytes.js";

const HTTP_RETRY_ATTEMPTS = 3;
const HTTP_RETRY_BASE_MS = 250;

export class HttpError extends Error {
  readonly status: number;
  constructor(status: number, message: string) {
    super(message);
    this.name = "HttpError";
    this.status = status;
  }
}

function sleep(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms));
}

function isAbortError(err: unknown): boolean {
  return (
    err instanceof DOMException &&
    (err.name === "AbortError" || err.code === DOMException.ABORT_ERR)
  );
}

function isRetryableStatus(status: number): boolean {
  return status >= 500 && status < 600;
}

export interface HttpFetchOpts {
  retries?: number;
  baseDelayMs?: number;
  fetchImpl?: typeof fetch;
}

/**
 * fetch with exponential-backoff retry on 5xx/network errors. 4xx responses
 * and aborts are returned/raised immediately (retrying a client error or a
 * user cancellation only wastes time).
 */
export async function httpFetch(
  url: string,
  init: RequestInit = {},
  { retries = HTTP_RETRY_ATTEMPTS, baseDelayMs = HTTP_RETRY_BASE_MS, fetchImpl = fetch }: HttpFetchOpts = {},
): Promise<Response> {
  let lastError: unknown = null;
  for (let attempt = 0; attempt < retries; attempt++) {
    try {
      const res = await fetchImpl(url, init);
      if (res.ok || !isRetryableStatus(res.status)) return res;
      lastError = new HttpError(res.status, `HTTP ${res.status}: ${await res.text().catch(() => "")}`);
    } catch (err) {
      if (isAbortError(err)) throw err;
      lastError = err;
    }
    if (attempt < retries - 1) {
      await sleep(baseDelayMs * Math.pow(2, attempt));
    }
  }
  throw lastError instanceof Error ? lastError : new Error("HTTP request failed");
}

/**
 * The signing surface needed to authenticate a provider request. Matches the
 * subset of PAPI's `PolkadotSigner` we use, so both a derived dev signer and a
 * browser wallet extension satisfy it. `signBytes` is async and — for wallets
 * and PAPI signers alike — wraps the payload in `<Bytes>…</Bytes>`; the provider
 * accepts that wrapped form (see `wrap_bytes` in crates/providers/auth).
 */
export interface ProviderRequestSigner {
  publicKey: Uint8Array;
  signBytes(input: Uint8Array): Promise<Uint8Array>;
}

/**
 * Build the `Authorization` header the provider node verifies via the
 * crates/providers/auth crate: header `Web3Storage <pubkey>:<sig>:<timestamp>`
 * over the message `web3storage:<METHOD>:<bucket_id>:<timestamp>`.
 *
 * Signs through `signBytes` so wallet-backed signers work — no raw key needed.
 */
export async function signProviderRequest(
  signer: ProviderRequestSigner,
  method: string,
  bucketId: bigint | number,
): Promise<Record<string, string>> {
  const timestamp = Math.floor(Date.now() / 1000).toString();
  // Interpolate the id directly (bigint/number both render as the decimal
  // string the provider reconstructs); `Number(bigint)` would lose precision above
  // 2^53 and break signature verification for large bucket ids.
  const message = `web3storage:${method}:${bucketId}:${timestamp}`;
  const sig = await signer.signBytes(new TextEncoder().encode(message));
  const pubHex = toHex(signer.publicKey);
  const sigHex = toHex(sig);
  return { Authorization: `Web3Storage ${pubHex}:${sigHex}:${timestamp}` };
}

// ── Off-chain agreement-term negotiation ────────────────────────────────────
// The off-chain half of the negotiate -> establish flow (#105). The bucket
// owner POSTs a quote to the provider node's /negotiate endpoint; the provider
// signs AgreementTerms and returns them as SignedTerms, which the owner then
// redeems on-chain via establish_storage_agreement / create_drive /
// create_s3_bucket. Pure HTTP here — the SCALE/Enum shaping of the response
// lives in layer0 (buildSignedTermsArgs), so core stays chain-free.

/** Replica-sync parameters carried by replica agreement terms. */
export interface ReplicaTermsWire {
  sync_balance: bigint | number | string;
  min_sync_interval: number;
  /**
   * Per-byte sync price (runtime v0.3.0+). Optional on the request — the
   * provider quotes it, like `price_per_byte` — and always present on the
   * provider-signed terms.
   */
  sync_price?: bigint | number | string;
}

/** A storage quote the provider is asked to sign (JSON wire shape). */
export interface NegotiateRequest {
  owner: string;
  max_bytes: bigint | number | string;
  duration: number;
  price_per_byte?: bigint | number | string;
  bucket_id?: bigint | number | string | null;
  replica_params?: ReplicaTermsWire | null;
}

/** Provider-signed terms returned by POST /negotiate. */
export interface SignedTerms {
  terms: {
    owner: string;
    max_bytes: bigint | number | string;
    duration: number;
    price_per_byte: bigint | number | string;
    valid_until: number;
    nonce: bigint | number | string;
    bucket_id?: bigint | number | string | null;
    replica_params?: ReplicaTermsWire | null;
  };
  /** SCALE-encoded MultiSignature as 0x-hex, e.g. `0x01<64-byte sr25519 sig>`. */
  signature: string;
}

/**
 * POST /negotiate and return the provider-signed terms. bigint fields are
 * serialized as decimal strings (the provider's serde accepts string-or-number
 * for the u64/u128 fields). Single attempt by default: /negotiate allocates a
 * provider-side nonce, so retrying a transient 5xx would waste nonces.
 */
export async function negotiateTerms(
  providerUrl: string,
  request: NegotiateRequest,
  opts: HttpFetchOpts = {},
): Promise<SignedTerms> {
  const base = providerUrl.replace(/\/+$/, "");
  const res = await httpFetch(
    `${base}/negotiate`,
    {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(request, (_k, v) => (typeof v === "bigint" ? v.toString() : v)),
    },
    { retries: 1, ...opts },
  );
  if (!res.ok) {
    throw new HttpError(
      res.status,
      `/negotiate failed: ${res.status} ${await res.text().catch(() => "")}`,
    );
  }
  return (await res.json()) as SignedTerms;
}
