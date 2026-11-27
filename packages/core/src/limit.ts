// SPDX-License-Identifier: Apache-2.0

/**
 * Limit how many async calls run at once. `createLimiter(n)` returns a
 * function that runs `fn` when fewer than `n` calls are active. Do not
 * await a limited call from inside another limited call of the same
 * limiter: that can deadlock.
 */
export function createLimiter(n: number): <T>(fn: () => Promise<T>) => Promise<T> {
  let active = 0;
  const queue: (() => void)[] = [];
  return async <T>(fn: () => Promise<T>): Promise<T> => {
    // A waiter is woken with the slot already counted for it (see `finally`),
    // so a caller arriving in between cannot take it.
    if (active >= n) await new Promise<void>((resolve) => queue.push(resolve));
    else active++;
    try {
      return await fn();
    } finally {
      const next = queue.shift();
      if (next) next();
      else active--;
    }
  };
}
