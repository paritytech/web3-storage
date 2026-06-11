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
