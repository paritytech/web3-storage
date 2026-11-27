// SPDX-License-Identifier: Apache-2.0

import { describe, expect, it } from "vitest";

import { createLimiter } from "./limit.js";

describe("createLimiter", () => {
  it("never runs more than n calls at once, even when a call starts as one ends", async () => {
    const limit = createLimiter(2);
    let active = 0;
    let peak = 0;
    const task = async () => {
      active++;
      peak = Math.max(peak, active);
      await Promise.resolve();
      active--;
    };
    const runs: Promise<void>[] = [];
    for (let i = 0; i < 6; i++) runs.push(limit(task));
    // A new call arrives in the same tick a slot is released.
    runs.push(runs[0].then(() => limit(task)));
    await Promise.all(runs);
    expect(peak).toBe(2);
  });

  it("releases the slot when the call throws", async () => {
    const limit = createLimiter(1);
    await expect(limit(async () => { throw new Error("boom"); })).rejects.toThrow("boom");
    await expect(limit(async () => 7)).resolves.toBe(7);
  });
});
