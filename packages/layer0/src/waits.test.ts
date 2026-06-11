import { Subject } from "rxjs";
import { describe, expect, it, vi } from "vitest";
import { firstMatch } from "./waits.js";

describe("firstMatch", () => {
  it("skips non-matching emissions and resolves with the first match", async () => {
    const source = new Subject<number>();
    const p = firstMatch(source, (n) => n > 2, { timeoutMs: 1_000 });
    source.next(1);
    source.next(2);
    source.next(3);
    await expect(p).resolves.toBe(3);
  });

  it("times out with the description in the error", async () => {
    const source = new Subject<number>();
    await expect(
      firstMatch(source, () => false, { timeoutMs: 20, description: "the thing" }),
    ).rejects.toThrow("Timed out after 20ms waiting for the thing");
  });

  it("drives onTick while waiting", async () => {
    vi.useFakeTimers();
    const source = new Subject<number>();
    const ticks: number[] = [];
    const p = firstMatch(source, (n) => n === 1, {
      timeoutMs: 5_000,
      tickMs: 100,
      onTick: (ms) => ticks.push(ms),
    });
    await vi.advanceTimersByTimeAsync(350);
    source.next(1);
    await p;
    vi.useRealTimers();
    expect(ticks.length).toBe(3);
  });
});
