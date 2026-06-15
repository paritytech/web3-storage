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

/** The keypair surface needed to sign provider requests (sr25519). */
export interface SigningKeypair {
  publicKey: Uint8Array;
  sign(input: Uint8Array): Uint8Array;
}

/**
 * Build the `Authorization` header the provider node verifies in
 * provider-node/src/auth.rs: header `Web3Storage <pubkey>:<sig>:<timestamp>`
 * over the message `web3storage:<METHOD>:<bucket_id>:<timestamp>` — a RAW
 * sr25519 signature (no `<Bytes>` wrapping), which is why this takes a
 * keypair rather than a PolkadotSigner (signBytes may wrap).
 */
export function signProviderRequest(
  keypair: SigningKeypair,
  method: string,
  bucketId: bigint | number,
): Record<string, string> {
  const timestamp = Math.floor(Date.now() / 1000).toString();
  const message = `web3storage:${method}:${Number(bucketId)}:${timestamp}`;
  const sig = keypair.sign(new TextEncoder().encode(message));
  const pubHex = toHex(keypair.publicKey);
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
