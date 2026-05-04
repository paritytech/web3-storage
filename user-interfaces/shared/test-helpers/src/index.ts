/**
 * Shared Playwright helpers for the three Web3-Storage UIs (drive-ui,
 * console-ui, provider). Modeled after polkadot-bulletin-chain/console-ui's
 * `e2e/utils.ts` + `e2e/fixtures.ts`.
 */

import { test as base, expect, type Page } from "@playwright/test";

const PROVIDER_HEALTH_URL = process.env.PROVIDER_HEALTH_URL ?? "http://127.0.0.1:3333/health";

/**
 * Wait for the chain-connection indicator to appear (the `data-testid="block-number"`
 * element). All three UIs expose this once a finalized block has been received.
 */
export async function waitForConnection(page: Page, timeout = 30_000) {
  await expect(page.getByTestId("block-number")).toBeVisible({ timeout });
}

/**
 * Wait for the chain to produce enough blocks so mortal transactions have a
 * valid era. Submitting at block 1 on a freshly started --dev chain yields
 * "Stale" errors.
 */
export async function waitForMinBlock(page: Page, minBlock = 3, timeout = 30_000) {
  await expect(async () => {
    const text = await page.getByTestId("block-number").textContent();
    const num = parseInt(text?.replace(/[#,]/g, "") ?? "0", 10);
    expect(num).toBeGreaterThanOrEqual(minBlock);
  }).toPass({ timeout });
}

/**
 * Optionally probe the provider node /health endpoint before tests run; if
 * unreachable, fail with a clear hint instead of letting the test time out
 * waiting for a non-existent provider.
 *
 * Returns true on success, false otherwise. Tests that don't require the
 * provider can ignore the result.
 */
export async function probeProviderHealth(): Promise<boolean> {
  try {
    const res = await fetch(PROVIDER_HEALTH_URL);
    return res.ok;
  } catch {
    return false;
  }
}

/**
 * Build a Playwright `test` instance with a `localPage` fixture that:
 *  1. Pre-injects per-UI localStorage keys before any JS loads
 *  2. Navigates to "/"
 *  3. Waits for chain connection (block-number testid)
 *  4. Waits for block #3 to clear mortal-era issues
 *
 * Each UI calls this to get a customised fixture.
 */
export function makeLocalPageFixture(opts: {
  localStorage: Record<string, string>;
  baseURL?: string;
}) {
  return base.extend<{ localPage: Page }>({
    localPage: async ({ page }, use) => {
      await page.addInitScript((entries: Record<string, string>) => {
        for (const [k, v] of Object.entries(entries)) {
          localStorage.setItem(k, v);
        }
      }, opts.localStorage);
      await page.goto(opts.baseURL ?? "/");
      await waitForConnection(page);
      await waitForMinBlock(page);
      await use(page);
    },
  });
}

export { expect };
