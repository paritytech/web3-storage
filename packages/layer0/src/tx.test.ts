import { Subject } from "rxjs";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  formatDispatchError,
  NonceManager,
  requireOneEvent,
  submitTx,
  TxDispatchError,
  type SubmittableTx,
  type TxStatusUpdate,
} from "./tx.js";

afterEach(() => vi.useRealTimers());

function fakeTx(): { tx: SubmittableTx; stream: Subject<any>; signed: number } {
  const stream = new Subject<any>();
  const holder = { signed: 0 };
  const tx = {
    signSubmitAndWatch: () => {
      holder.signed += 1;
      return stream as never;
    },
  } as unknown as SubmittableTx;
  return { tx, stream, get signed() { return holder.signed; } } as never;
}

const signer = {} as never;

/** submitOnce subscribes async (microtask hops) — let it attach first. */
const tick = () => new Promise((r) => setTimeout(r, 0));

describe("submitTx", () => {
  it("resolves at best-block inclusion with the in-block events", async () => {
    const { tx, stream } = fakeTx();
    const statuses: TxStatusUpdate[] = [];
    const p = submitTx(tx, signer, { label: "t", onStatus: (u) => statuses.push(u) });
    await tick();
    stream.next({ type: "signed" });
    stream.next({ type: "broadcasted", txHash: "0x01" });
    stream.next({ type: "txBestBlocksState", found: true, ok: true, block: { hash: "0xb" }, events: [1] });
    const result = await p;
    expect((result as any).events).toEqual([1]);
    expect(statuses.map((s) => s.phase)).toEqual(["signed", "in-pool", "best"]);
    expect(statuses.at(-1)!.final).toBe(true);
  });

  it("rejects with TxDispatchError when the dispatch fails", async () => {
    const { tx, stream } = fakeTx();
    const p = submitTx(tx, signer, { label: "t", onStatus: null });
    const guarded = p.catch((e) => e);
    await tick();
    stream.next({
      type: "txBestBlocksState",
      found: true,
      ok: false,
      dispatchError: { type: "Module", value: { type: "StorageProvider", value: { type: "Nope" } } },
    });
    const err = await guarded;
    expect(err).toBeInstanceOf(TxDispatchError);
    expect((err as Error).message).toContain("Module::StorageProvider::Nope");
  });

  it("waits for finalization in finalized mode", async () => {
    const { tx, stream } = fakeTx();
    let settled = false;
    const p = submitTx(tx, signer, { mode: "finalized", label: "t", onStatus: null }).then((r) => {
      settled = true;
      return r;
    });
    await tick();
    stream.next({ type: "txBestBlocksState", found: true, ok: true, block: { hash: "0xb" }, events: [] });
    await tick();
    expect(settled).toBe(false);
    stream.next({ type: "finalized", ok: true, block: { hash: "0xf" }, events: [] });
    await p;
    expect(settled).toBe(true);
  });

  it("retries on stale-nonce stream errors, then succeeds", async () => {
    vi.useFakeTimers();
    let attempt = 0;
    let current = new Subject<any>();
    const tx = {
      signSubmitAndWatch: () => {
        attempt += 1;
        current = new Subject<any>();
        return current as never;
      },
    } as unknown as SubmittableTx;
    const p = submitTx(tx, signer, { label: "t", onStatus: null });
    current.error(new Error("Invalid Transaction: Stale"));
    await vi.advanceTimersByTimeAsync(6_500);
    current.next({ type: "txBestBlocksState", found: true, ok: true, block: { hash: "0xb" }, events: [] });
    await p;
    expect(attempt).toBe(2);
  });

  it("does not retry when retryStale is 0", async () => {
    const { tx, stream } = fakeTx();
    const p = submitTx(tx, signer, { label: "t", retryStale: 0, onStatus: null });
    const guarded = p.catch((e) => e);
    stream.error(new Error("Invalid Transaction: Stale"));
    expect(String(await guarded)).toContain("Stale");
  });
});

describe("requireOneEvent", () => {
  const descriptor = (payloads: unknown[]) => ({
    filter: () => payloads.map((payload) => ({ payload })),
  });

  it("returns the single payload", () => {
    expect(requireOneEvent([], descriptor(["x"]) as never, "X")).toBe("x");
  });

  it("throws on zero or many", () => {
    expect(() => requireOneEvent([], descriptor([]) as never, "X")).toThrow("got 0");
    expect(() => requireOneEvent([], descriptor(["a", "b"]) as never, "X")).toThrow("got 2");
  });
});

describe("NonceManager", () => {
  it("hands out sequential nonces", () => {
    const nm = new NonceManager(5);
    expect(nm.next()).toBe(5n);
    expect(nm.next()).toBe(6n);
  });

  it("reads the on-chain nonce at the best block", async () => {
    const api = {
      query: { System: { Account: { getValue: vi.fn(async () => ({ nonce: 9 })) } } },
    };
    const nm = await NonceManager.forAccount(api as never, "addr");
    expect(nm.next()).toBe(9n);
    expect(api.query.System.Account.getValue).toHaveBeenCalledWith("addr", { at: "best" });
  });
});

describe("formatDispatchError", () => {
  it("flattens nested module errors", () => {
    expect(
      formatDispatchError({ type: "Module", value: { type: "P", value: { type: "E" } } }),
    ).toBe("Module::P::E");
    expect(formatDispatchError("boom")).toBe("boom");
  });
});
