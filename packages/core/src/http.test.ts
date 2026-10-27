// SPDX-License-Identifier: Apache-2.0

import { afterEach, describe, expect, it, vi } from "vitest";
import { describeProviderError, httpErrorFromBody, httpFetch, HttpError, negotiateTerms, signProviderRequest } from "./http.js";
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

describe("provider error bodies", () => {
  it("parses code and details from a JSON 422", () => {
    const body = JSON.stringify({
      error: "max_bytes_below_minimum",
      details: { requested: 99, min_bytes: 100 },
    });
    const err = httpErrorFromBody(422, body, "/negotiate failed");
    expect(err.status).toBe(422);
    expect(err.code).toBe("max_bytes_below_minimum");
    expect(err.details).toEqual({ requested: 99, min_bytes: 100 });
    expect(err.message).not.toContain("{");
  });

  it("keeps the status and raw text for a non-JSON body", () => {
    const err = httpErrorFromBody(502, "Bad Gateway", "/negotiate failed");
    expect(err.status).toBe(502);
    expect(err.code).toBeUndefined();
    expect(err.details).toBeUndefined();
    expect(err.message).toBe("/negotiate failed: 502 Bad Gateway");
  });

  it("names the requested size and the minimum", () => {
    const msg = describeProviderError("max_bytes_below_minimum", { requested: 99, min_bytes: 100 });
    expect(msg).toContain("99 bytes");
    expect(msg).toContain("100 bytes");
  });

  it("describes a zero-byte request", () => {
    expect(describeProviderError("invalid_max_bytes_request")).toContain("0 bytes");
  });

  it("negotiateTerms throws an HttpError with the reason", async () => {
    const body = JSON.stringify({ error: "invalid_max_bytes_request" });
    const fetchImpl = vi.fn(async () => new Response(body, { status: 422 }));
    const err = await negotiateTerms("http://x", {} as never, { fetchImpl }).catch((e) => e);
    expect(err).toBeInstanceOf(HttpError);
    expect((err as HttpError).code).toBe("invalid_max_bytes_request");
    expect((err as HttpError).message).toContain("0 bytes");
  });
});
