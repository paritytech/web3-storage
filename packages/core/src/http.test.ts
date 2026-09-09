// SPDX-License-Identifier: Apache-2.0

import { afterEach, describe, expect, it, vi } from "vitest";
import { httpFetch, HttpError, signProviderRequest } from "./http.js";
import { hexToBytes } from "./bytes.js";

afterEach(() => vi.useRealTimers());

function res(status: number, body = ""): Response {
  return new Response(body, { status });
}

describe("httpFetch", () => {
  it("returns immediately on 2xx", async () => {
    const fetchImpl = vi.fn(async () => res(200, "ok"));
    const r = await httpFetch("http://x/", {}, { fetchImpl });
    expect(r.status).toBe(200);
    expect(fetchImpl).toHaveBeenCalledTimes(1);
  });

  it("does not retry 4xx", async () => {
    const fetchImpl = vi.fn(async () => res(404));
    const r = await httpFetch("http://x/", {}, { fetchImpl });
    expect(r.status).toBe(404);
    expect(fetchImpl).toHaveBeenCalledTimes(1);
  });

  it("retries 5xx with backoff and surfaces HttpError after exhaustion", async () => {
    vi.useFakeTimers();
    const fetchImpl = vi.fn(async () => res(503, "overloaded"));
    const p = httpFetch("http://x/", {}, { fetchImpl, retries: 3, baseDelayMs: 100 });
    const guarded = p.catch((e) => e);
    await vi.advanceTimersByTimeAsync(100 + 200);
    const err = await guarded;
    expect(err).toBeInstanceOf(HttpError);
    expect((err as HttpError).status).toBe(503);
    expect(fetchImpl).toHaveBeenCalledTimes(3);
  });

  it("recovers when a retry succeeds", async () => {
    vi.useFakeTimers();
    let calls = 0;
    const fetchImpl = vi.fn(async () => (++calls < 2 ? res(500) : res(200, "fine")));
    const p = httpFetch("http://x/", {}, { fetchImpl, baseDelayMs: 50 });
    await vi.advanceTimersByTimeAsync(50);
    const r = await p;
    expect(r.status).toBe(200);
    expect(fetchImpl).toHaveBeenCalledTimes(2);
  });

  it("propagates aborts without retrying", async () => {
    const fetchImpl = vi.fn(async () => {
      throw new DOMException("aborted", "AbortError");
    });
    await expect(httpFetch("http://x/", {}, { fetchImpl })).rejects.toThrow("aborted");
    expect(fetchImpl).toHaveBeenCalledTimes(1);
  });
});

describe("signProviderRequest", () => {
  it("builds the Web3Storage header over the canonical message", async () => {
    const seen: Uint8Array[] = [];
    const signer = {
      publicKey: hexToBytes("0xaa".repeat(1) + "bb".repeat(31)),
      signBytes: async (input: Uint8Array) => {
        seen.push(input);
        return hexToBytes("0x" + "cd".repeat(64));
      },
    };
    const headers = await signProviderRequest(signer, "PUT", 42n);
    const auth = headers.Authorization;
    expect(auth).toMatch(/^Web3Storage 0x[0-9a-f]{64}:0x[0-9a-f]{128}:\d+$/);
    const ts = auth.split(":").pop()!;
    expect(new TextDecoder().decode(seen[0])).toBe(`web3storage:PUT:42:${ts}`);
  });
});
